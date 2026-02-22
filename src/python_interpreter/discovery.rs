//! Python interpreter discovery.
//!
//! This module handles finding Python interpreters on the host machine,
//! including platform-specific discovery for Windows (py launcher, conda)
//! and Unix (pythonX.Y binaries, pyenv fallback).

use super::abiflags::fun_with_abiflags;
use super::{
    InterpreterConfig, InterpreterKind, MINIMUM_PYPY_MINOR, MINIMUM_PYTHON_MINOR, PythonInterpreter,
};
use crate::target::Arch;
use crate::{BridgeModel, Target};
use anyhow::{Context, Result, bail, format_err};
use pep440_rs::{Version, VersionSpecifiers};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str;
use tracing::{debug, instrument};

/// This snippet will give us information about the python interpreter's
/// version and abi as json through stdout
const GET_INTERPRETER_METADATA: &str = include_str!("get_interpreter_metadata.py");

/// The output format of [GET_INTERPRETER_METADATA]
#[derive(Deserialize)]
pub(super) struct InterpreterMetadataMessage {
    pub implementation_name: String,
    pub executable: Option<String>,
    pub major: usize,
    pub minor: usize,
    pub abiflags: Option<String>,
    pub interpreter: String,
    pub ext_suffix: Option<String>,
    // comes from `sysconfig.get_platform()`
    pub platform: String,
    // comes from `platform.system()`
    pub system: String,
    pub soabi: Option<String>,
    pub gil_disabled: bool,
}

// ---------------------------------------------------------------------------
// Windows discovery
// ---------------------------------------------------------------------------

/// Manages interpreter discovery on Windows.
///
/// Replaces the old `maybe_add_interp!` macro with explicit state,
/// deduplicating by `(major, minor, gil_disabled)` and tracking
/// seen executable paths to avoid duplicate warnings (#2751).
struct WindowsInterpreterFinder<'a> {
    target: &'a Target,
    bridge: &'a BridgeModel,
    min_python_minor: usize,
    requires_python: Option<&'a VersionSpecifiers>,
    versions_found: HashSet<(usize, usize, bool)>,
    seen_executables: HashSet<PathBuf>,
    interpreters: Vec<PythonInterpreter>,
}

impl<'a> WindowsInterpreterFinder<'a> {
    fn new(
        target: &'a Target,
        bridge: &'a BridgeModel,
        requires_python: Option<&'a VersionSpecifiers>,
    ) -> Self {
        Self {
            target,
            bridge,
            min_python_minor: bridge.minimal_python_minor_version(),
            requires_python,
            versions_found: HashSet::new(),
            seen_executables: HashSet::new(),
            interpreters: Vec::new(),
        }
    }

    /// Try to add an interpreter from the given executable path.
    ///
    /// Checks version constraints, deduplicates by version, and tracks
    /// seen executables to avoid processing the same interpreter twice.
    fn try_add(&mut self, executable: &Path) -> Result<()> {
        let interp = PythonInterpreter::check_executable(executable, self.target, self.bridge)?;
        if let Some(interp) = interp {
            let key = (interp.major, interp.minor, interp.gil_disabled);
            if interp.major == 3
                && interp.minor >= self.min_python_minor
                && !self.versions_found.contains(&key)
                && self.requires_python.is_none_or(|req| {
                    req.contains(&Version::new([interp.major as u64, interp.minor as u64]))
                })
            {
                // Track the canonical path of accepted interpreters
                if let Ok(canonical) = interp.executable.canonicalize() {
                    self.seen_executables.insert(canonical);
                }
                self.versions_found.insert(key);
                self.interpreters.push(interp);
            }
        }
        Ok(())
    }

    /// Check if an executable has already been seen (accepted or rejected).
    /// Returns `true` if already processed; marks it as seen if not.
    fn mark_seen(&mut self, executable: &Path) -> bool {
        if let Ok(canonical) = executable.canonicalize() {
            !self.seen_executables.insert(canonical)
        } else {
            !self.seen_executables.insert(executable.to_path_buf())
        }
    }

    fn into_interpreters(self) -> Vec<PythonInterpreter> {
        self.interpreters
    }
}

/// On windows regular Python installs are supported along with environments
/// being managed by `conda`.
///
/// We can't use the linux trick with trying different binary names since on
/// windows the binary is always called "python.exe".  However, whether dealing
/// with regular Python installs or `conda` environments there are tools we can
/// use to query the information regarding installed interpreters.
///
/// Regular Python installs downloaded from Python.org will include the python
/// launcher by default.  We can use the launcher to find the information we need
/// for each installed interpreter using `py -0` which produces something like
/// the following output (the path can by determined using `sys.executable`):
///
/// ```bash
/// Installed Pythons found by py Launcher for Windows
/// -3.7-64 *
/// -3.6-32
/// ```
///
/// When using `conda` we can use the `conda info -e` command to retrieve information
/// regarding the installed interpreters being managed by `conda`.  This is an example
/// of the output expected:
///
/// ```bash
/// # conda environments:
/// #
/// base                     C:\Users\<user-name>\Anaconda3
/// foo1                  *  C:\Users\<user-name>\Anaconda3\envs\foo1
/// foo2                  *  C:\Users\<user-name>\Anaconda3\envs\foo2
/// ```
///
/// The information required can either by obtained by parsing this output directly or
/// by invoking the interpreters to obtain the information.
///
/// As well as the version numbers, etc. of the interpreters we also have to find the
/// pointer width to make sure that the pointer width (32-bit or 64-bit) matches across
/// platforms.
pub(super) fn find_all_windows(
    target: &Target,
    bridge: &BridgeModel,
    requires_python: Option<&VersionSpecifiers>,
) -> Result<Vec<PythonInterpreter>> {
    let mut finder = WindowsInterpreterFinder::new(target, bridge, requires_python);

    // If Python is installed from Python.org it should include the "python launcher"
    // which is used to find the installed interpreters
    let execution = Command::new("cmd")
        .arg("/c")
        .arg("py")
        .arg("--list-paths")
        .output();
    if let Ok(output) = execution {
        // x86_64: ' -3.10-64 * C:\Users\xxx\AppData\Local\Programs\Python\Python310\python.exe'
        // x86_64: ' -3.11 * C:\Users\xxx\AppData\Local\Programs\Python\Python310\python.exe'
        // arm64:  ' -V:3.11-arm64 * C:\Users\xxx\AppData\Local\Programs\Python\Python311-arm64\python.exe
        let expr = Regex::new(r" -(V:)?(\d).(\d+)-?(arm)?(\d*)\s*\*?\s*(.*)?").unwrap();
        let stdout = str::from_utf8(&output.stdout).unwrap();
        for line in stdout.lines() {
            if let Some(capture) = expr.captures(line) {
                let major = capture
                    .get(2)
                    .unwrap()
                    .as_str()
                    .parse::<usize>()
                    .context("Expected a digit for major version")?;
                let minor = capture
                    .get(3)
                    .unwrap()
                    .as_str()
                    .parse::<usize>()
                    .context("Expected a digit for minor version")?;
                if !finder.versions_found.contains(&(major, minor, false))
                    && !finder.versions_found.contains(&(major, minor, true))
                {
                    let executable = capture.get(6).unwrap().as_str();
                    let executable_path = Path::new(&executable);
                    // Skip non-existing paths
                    if !executable_path.exists() {
                        continue;
                    }
                    if finder.mark_seen(executable_path) {
                        continue;
                    }
                    finder.try_add(executable_path)?;
                }
            }
        }
    }

    // Conda environments are also supported on windows
    let conda_info = Command::new("conda").arg("info").arg("-e").output();
    if let Ok(output) = conda_info {
        let lines = str::from_utf8(&output.stdout).unwrap().lines();
        // The regex has three parts: The first matches the name and skips
        // comments, the second skips the part in between and the third
        // extracts the path
        let re = Regex::new(r"^([^#].*?)[\s*]+([\w\\:.-]+)\s*$").unwrap();
        let mut paths = vec![];
        for i in lines {
            if let Some(capture) = re.captures(i) {
                if &capture[1] == "base" {
                    continue;
                }
                paths.push(String::from(&capture[2]));
            }
        }

        for path in paths {
            let executable_win = Path::new(&path).join("python.exe");
            let executable = if executable_win.exists() {
                executable_win
            } else {
                Path::new(&path).join("python")
            };
            if !finder.mark_seen(&executable) {
                finder.try_add(executable.as_path())?;
            }
        }
    }

    // Fallback to pythonX.Y for Microsoft Store versions
    for minor in finder.min_python_minor..=bridge.maximum_python_minor_version() {
        let key = (3, minor, false);
        if !finder.versions_found.contains(&key) {
            let executable = format!("python3.{minor}.exe");
            finder.try_add(Path::new(&executable))?;
        }
    }

    let interpreters = finder.into_interpreters();
    if interpreters.is_empty() {
        bail!(
            "Could not find any interpreters, are you sure you have python installed on your PATH?"
        );
    };
    Ok(interpreters)
}

// ---------------------------------------------------------------------------
// Cross-platform discovery
// ---------------------------------------------------------------------------

impl PythonInterpreter {
    /// Checks whether the given command is a python interpreter and returns a
    /// [PythonInterpreter] if that is the case
    #[instrument(skip_all, fields(executable = %executable.as_ref().display()))]
    pub fn check_executable(
        executable: impl AsRef<Path>,
        target: &Target,
        bridge: &BridgeModel,
    ) -> Result<Option<PythonInterpreter>> {
        let output = Command::new(executable.as_ref())
            .env("PYTHONNOUSERSITE", "1")
            .args(["-c", GET_INTERPRETER_METADATA])
            .output();

        let err_msg = format!(
            "Trying to get metadata from the python interpreter '{}' failed",
            executable.as_ref().display()
        );
        let output = match output {
            Ok(output) => {
                if output.status.success() {
                    output
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if stderr.starts_with(&format!(
                        "pyenv: {}: command not found",
                        executable.as_ref().display()
                    )) {
                        eprintln!(
                            "⚠️  Warning: skipped unavailable python interpreter '{}' from pyenv",
                            executable.as_ref().display()
                        );
                        return Ok(None);
                    } else {
                        eprintln!("{stderr}");
                        bail!(err_msg);
                    }
                }
            }
            Err(err) => {
                if err.kind() == io::ErrorKind::NotFound {
                    if cfg!(windows) {
                        if let Some(python) = executable.as_ref().to_str() {
                            let ver = if python.starts_with("python") {
                                python.strip_prefix("python").unwrap_or(python)
                            } else {
                                python
                            };
                            // Try py -x.y on Windows
                            let mut metadata_py = tempfile::NamedTempFile::new()?;
                            write!(metadata_py, "{GET_INTERPRETER_METADATA}")?;
                            let mut cmd = Command::new("cmd");
                            let suffix = match target.target_arch() {
                                Arch::X86 => "-32",
                                Arch::X86_64 => "-64",
                                Arch::Aarch64 => "-arm64",
                                _ => "",
                            };
                            cmd.arg("/c")
                                .arg("py")
                                .arg(format!("-{ver}{suffix}"))
                                .arg(metadata_py.path())
                                .env("PYTHONNOUSERSITE", "1");
                            let output = cmd.output();
                            match output {
                                Ok(output) if output.status.success() => output,
                                _ => return Ok(None),
                            }
                        } else {
                            return Ok(None);
                        }
                    } else {
                        return Ok(None);
                    }
                } else {
                    return Err(err).context(err_msg);
                }
            }
        };
        let message: InterpreterMetadataMessage = serde_json::from_slice(&output.stdout)
            .context(err_msg)
            .context(String::from_utf8_lossy(&output.stdout).trim().to_string())?;

        Self::from_metadata_message(executable, target, bridge, message)
    }

    /// Configure a `PythonInterpreter` from the metadata message.
    ///
    /// Returns `None` if the interpreter is not suitable to use (e.g. too old or wrong architecture)
    fn from_metadata_message(
        executable: impl AsRef<Path>,
        target: &Target,
        bridge: &BridgeModel,
        message: InterpreterMetadataMessage,
    ) -> Result<Option<PythonInterpreter>> {
        if (message.major == 2 && message.minor != 7) || (message.major == 3 && message.minor < 5) {
            debug!(
                "Skipping outdated python interpreter '{}'",
                executable.as_ref().display()
            );
            return Ok(None);
        }

        let interpreter = match message.interpreter.as_str() {
            "cpython" => InterpreterKind::CPython,
            "pypy" => InterpreterKind::PyPy,
            "graalvm" | "graalpy" => InterpreterKind::GraalPy,
            other => {
                bail!("Unsupported interpreter {}", other);
            }
        };

        let abiflags = fun_with_abiflags(&message, target, bridge).context(format_err!(
            "Failed to get information from the python interpreter at {}",
            executable.as_ref().display()
        ))?;

        let executable = message
            .executable
            .map(PathBuf::from)
            .unwrap_or_else(|| executable.as_ref().to_path_buf());

        if target.is_windows() {
            'windows_arch_check: {
                // on windows we must check the architecture, because three different architectures
                // can all run on the same hardware
                let python_arch = match message.platform.as_str().trim() {
                    "win32" => Arch::X86,
                    "win-amd64" => Arch::X86_64,
                    "win-arm64" => Arch::Aarch64,
                    _ => {
                        eprintln!(
                            "⚠️  Warning: '{}' reports unknown platform. This may fail to build.",
                            executable.display()
                        );
                        break 'windows_arch_check;
                    }
                };

                if python_arch != target.target_arch() {
                    eprintln!(
                        "👽 '{}' reports a platform '{platform}' (architecture '{python_arch}'), while the Rust target is '{target_arch}'. Skipping.",
                        executable.display(),
                        platform = message.platform,
                        python_arch = python_arch,
                        target_arch = target.target_arch(),
                    );
                    return Ok(None);
                }
            }
        }

        let platform = if message.platform.starts_with("macosx") {
            // We don't use platform from sysconfig on macOS
            None
        } else {
            Some(message.platform.to_lowercase().replace(['-', '.'], "_"))
        };

        debug!(
            "Found {} interpreter at {}",
            interpreter,
            executable.display()
        );
        Ok(Some(PythonInterpreter {
            config: InterpreterConfig {
                major: message.major,
                minor: message.minor,
                interpreter_kind: interpreter,
                abiflags,
                ext_suffix: message
                    .ext_suffix
                    .context("syconfig didn't define an `EXT_SUFFIX` ಠ_ಠ")?,
                pointer_width: None,
                gil_disabled: message.gil_disabled,
            },
            executable,
            platform,
            runnable: true,
            implementation_name: message.implementation_name,
            soabi: message.soabi,
        }))
    }

    /// Find all available python interpreters for a given target
    pub fn find_by_target(
        target: &Target,
        requires_python: Option<&VersionSpecifiers>,
        bridge: Option<&BridgeModel>,
    ) -> Vec<PythonInterpreter> {
        let min_python_minor = bridge
            .map(|bridge| bridge.minimal_python_minor_version())
            .unwrap_or(MINIMUM_PYTHON_MINOR);
        let min_pypy_minor = bridge
            .map(|bridge| bridge.minimal_pypy_minor_version())
            .unwrap_or(MINIMUM_PYPY_MINOR);
        let supports_free_threaded = bridge
            .map(|bridge| bridge.supports_free_threaded())
            .unwrap_or(false);
        InterpreterConfig::lookup_target(target)
            .into_iter()
            .filter_map(|config| match requires_python {
                Some(requires_python) => {
                    if requires_python
                        .contains(&Version::new([config.major as u64, config.minor as u64]))
                    {
                        Some(Self::from_config(config))
                    } else {
                        None
                    }
                }
                None => Some(Self::from_config(config)),
            })
            .filter_map(|config| match config.interpreter_kind {
                InterpreterKind::CPython => {
                    if config.minor >= min_python_minor {
                        Some(config)
                    } else {
                        None
                    }
                }
                InterpreterKind::PyPy => {
                    if config.minor >= min_pypy_minor {
                        Some(config)
                    } else {
                        None
                    }
                }
                InterpreterKind::GraalPy => Some(config),
            })
            .filter_map(|config| {
                if config.gil_disabled && !supports_free_threaded {
                    None
                } else {
                    Some(config)
                }
            })
            .collect()
    }

    /// Tries to find all installed python versions using the heuristic for the
    /// given platform.
    ///
    /// We have two filters: The optional requires-python from the pyproject.toml and minimum python
    /// minor either from the bindings (i.e. Cargo.toml `abi3-py{major}{minor}`) or the global
    /// default minimum minor version
    pub fn find_all(
        target: &Target,
        bridge: &BridgeModel,
        requires_python: Option<&VersionSpecifiers>,
    ) -> Result<Vec<PythonInterpreter>> {
        if target.is_windows() {
            // TOFIX: add PyPy support to Windows
            return find_all_windows(target, bridge, requires_python);
        };

        let mut executables: Vec<String> = (bridge.minimal_python_minor_version()
            ..=bridge.maximum_python_minor_version())
            .filter(|minor| {
                requires_python
                    .map(|requires_python| {
                        requires_python.contains(&Version::new([3, *minor as u64]))
                    })
                    .unwrap_or(true)
            })
            .map(|minor| format!("python3.{minor}"))
            .collect();

        // Also try to find PyPy for cffi and pyo3 bindings
        if *bridge == BridgeModel::Cffi || bridge.is_pyo3() {
            executables.extend(
                (bridge.minimal_pypy_minor_version()..=bridge.maximum_pypy_minor_version())
                    .filter(|minor| {
                        requires_python
                            .map(|requires_python| {
                                requires_python.contains(&Version::new([3, *minor as u64]))
                            })
                            .unwrap_or(true)
                    })
                    .map(|minor| format!("pypy3.{minor}")),
            );
        }

        // Deduplicate by (kind, major, minor, gil_disabled) to avoid
        // picking up the same interpreter via multiple names.
        let mut seen = HashSet::new();
        let mut available_versions = Vec::new();
        for executable in executables {
            if let Some(version) = PythonInterpreter::check_executable(executable, target, bridge)?
            {
                let key = (
                    version.interpreter_kind,
                    version.major,
                    version.minor,
                    version.gil_disabled,
                );
                if seen.insert(key) {
                    available_versions.push(version);
                }
            }
        }

        // Fallback: try `python3` and `python` for environments like pyenv
        // where only the generic name is available (fixes #2312)
        if available_versions.is_empty() {
            for name in &["python3", "python"] {
                if let Some(version) = PythonInterpreter::check_executable(name, target, bridge)?
                    .filter(|v| {
                        v.major == 3
                            && v.minor >= bridge.minimal_python_minor_version()
                            && requires_python.is_none_or(|req| {
                                req.contains(&Version::new([v.major as u64, v.minor as u64]))
                            })
                    })
                {
                    available_versions.push(version);
                    break;
                }
            }
        }

        Ok(available_versions)
    }

    /// Checks that given list of executables are all valid python interpreters,
    /// determines the abiflags and versions of those interpreters and
    /// returns them as [PythonInterpreter]
    pub fn check_executables(
        executables: &[PathBuf],
        target: &Target,
        bridge: &BridgeModel,
    ) -> Result<Vec<PythonInterpreter>> {
        let mut available_versions = Vec::new();
        let mut missing = Vec::new();
        for executable in executables {
            if let Some(version) = PythonInterpreter::check_executable(executable, target, bridge)
                .context(format!(
                "{} is not a valid python interpreter",
                executable.display()
            ))? {
                available_versions.push(version);
            } else {
                missing.push(executable);
            }
        }

        if !missing.is_empty() {
            let missing_str = missing
                .iter()
                .map(|p| format!("`{}`", p.display()))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "The following Python interpreters could not be found: {}",
                missing_str
            );
        }

        Ok(available_versions)
    }

    /// Run a python script using this Python interpreter.
    pub fn run_script(&self, script: &str) -> Result<String> {
        if !self.runnable {
            bail!("This {} isn't runnable", self);
        }
        let out = Command::new(&self.executable)
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .and_then(|mut child| {
                child
                    .stdin
                    .as_mut()
                    .expect("piped stdin")
                    .write_all(script.as_bytes())?;
                child.wait_with_output()
            });

        match out {
            Err(err) => {
                if err.kind() == io::ErrorKind::NotFound {
                    bail!(
                        "Could not find any interpreter at {}, \
                     are you sure you have Python installed on your PATH?",
                        self.executable.display()
                    );
                } else {
                    bail!(
                        "Failed to run the Python interpreter at {}: {}",
                        self.executable.display(),
                        err
                    );
                }
            }
            Ok(ok) if !ok.status.success() => bail!("Python script failed"),
            Ok(ok) => Ok(String::from_utf8(ok.stdout)?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{PyO3, PyO3Crate};
    use expect_test::expect;
    use insta::assert_snapshot;
    use std::str::FromStr;

    #[test]
    fn test_find_interpreter_by_target() {
        let target = Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap();
        let pythons = PythonInterpreter::find_by_target(&target, None, None)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let expected = expect![[r#"
            [
                "CPython 3.7m",
                "CPython 3.8",
                "CPython 3.9",
                "CPython 3.10",
                "CPython 3.11",
                "CPython 3.12",
                "CPython 3.13",
                "CPython 3.14",
                "PyPy 3.8",
                "PyPy 3.9",
                "PyPy 3.10",
                "PyPy 3.11",
            ]
        "#]];
        expected.assert_debug_eq(&pythons);

        // pyo3 0.23+ should find CPython 3.13t
        let pythons = PythonInterpreter::find_by_target(
            &target,
            None,
            Some(&BridgeModel::PyO3(PyO3 {
                crate_name: PyO3Crate::PyO3,
                version: semver::Version::new(0, 23, 0),
                abi3: None,
                metadata: None,
            })),
        )
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
        let expected = expect![[r#"
            [
                "CPython 3.7m",
                "CPython 3.8",
                "CPython 3.9",
                "CPython 3.10",
                "CPython 3.11",
                "CPython 3.12",
                "CPython 3.13",
                "CPython 3.14",
                "CPython 3.13t",
                "CPython 3.14t",
                "PyPy 3.9",
                "PyPy 3.10",
                "PyPy 3.11",
            ]
        "#]];
        expected.assert_debug_eq(&pythons);

        let pythons = PythonInterpreter::find_by_target(
            &target,
            Some(&VersionSpecifiers::from_str(">=3.8").unwrap()),
            None,
        )
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
        let expected = expect![[r#"
            [
                "CPython 3.8",
                "CPython 3.9",
                "CPython 3.10",
                "CPython 3.11",
                "CPython 3.12",
                "CPython 3.13",
                "CPython 3.14",
                "PyPy 3.8",
                "PyPy 3.9",
                "PyPy 3.10",
                "PyPy 3.11",
            ]
        "#]];
        expected.assert_debug_eq(&pythons);

        let pythons = PythonInterpreter::find_by_target(
            &target,
            Some(&VersionSpecifiers::from_str(">=3.10").unwrap()),
            None,
        )
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
        let expected = expect![[r#"
            [
                "CPython 3.10",
                "CPython 3.11",
                "CPython 3.12",
                "CPython 3.13",
                "CPython 3.14",
                "PyPy 3.10",
                "PyPy 3.11",
            ]
        "#]];
        expected.assert_debug_eq(&pythons);

        let pythons = PythonInterpreter::find_by_target(
            &target,
            Some(&VersionSpecifiers::from_str(">=3.8").unwrap()),
            Some(&BridgeModel::PyO3(PyO3 {
                crate_name: PyO3Crate::PyO3,
                version: semver::Version::new(0, 23, 0),
                abi3: None,
                metadata: None,
            })),
        )
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
        // should exclude PyPy < 3.9
        let expected = expect![[r#"
            [
                "CPython 3.8",
                "CPython 3.9",
                "CPython 3.10",
                "CPython 3.11",
                "CPython 3.12",
                "CPython 3.13",
                "CPython 3.14",
                "CPython 3.13t",
                "CPython 3.14t",
                "PyPy 3.9",
                "PyPy 3.10",
                "PyPy 3.11",
            ]
        "#]];
        expected.assert_debug_eq(&pythons);
    }

    #[test]
    fn test_interpreter_from_metadata_windows() {
        // Test cases for different scenarios
        let target_x64 = Target::from_resolved_target_triple("x86_64-pc-windows-msvc").unwrap();
        let target_x86 = Target::from_resolved_target_triple("i686-pc-windows-msvc").unwrap();
        let target_arm64 = Target::from_resolved_target_triple("aarch64-pc-windows-msvc").unwrap();

        let bridge = BridgeModel::PyO3(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version: semver::Version::new(0, 26, 0),
            abi3: None,
            metadata: None,
        });

        let message = |major, minor, platform: &str| InterpreterMetadataMessage {
            major,
            minor,
            interpreter: "cpython".to_string(),
            implementation_name: "CPython".to_string(),
            abiflags: None,
            ext_suffix: Some(".pyd".to_string()),
            platform: platform.to_string(),
            executable: None,
            soabi: None,
            gil_disabled: false,
            system: "windows".to_string(),
        };

        // Test Python 2.x should be rejected
        assert_eq!(
            PythonInterpreter::from_metadata_message(
                "python2.7",
                &target_x64,
                &bridge,
                message(2, 7, "win-amd64"),
            )
            .unwrap_err()
            .to_string(),
            "Failed to get information from the python interpreter at python2.7"
        );

        // Test Python 3.x but below minimum version
        assert_eq!(
            PythonInterpreter::from_metadata_message(
                "python3.6",
                &target_x64,
                &bridge,
                message(3, 6, "win-amd64"),
            )
            .unwrap_err()
            .to_string(),
            "Failed to get information from the python interpreter at python3.6"
        );

        // Test valid Python version with matching platform and architecture
        for (target, platform) in &[
            (&target_x86, "win32"),
            (&target_x64, "win-amd64"),
            (&target_arm64, "win-arm64"),
        ] {
            assert_eq!(
                PythonInterpreter::from_metadata_message(
                    "python3.10",
                    target,
                    &bridge,
                    message(3, 10, platform),
                )
                .unwrap()
                .unwrap(),
                PythonInterpreter {
                    config: InterpreterConfig {
                        major: 3,
                        minor: 10,
                        interpreter_kind: InterpreterKind::CPython,
                        abiflags: "".to_string(),
                        ext_suffix: ".pyd".to_string(),
                        pointer_width: None,
                        gil_disabled: false,
                    },
                    executable: PathBuf::from("python3.10"),
                    platform: Some(platform.replace("-", "_")),
                    runnable: true,
                    implementation_name: "CPython".to_string(),
                    soabi: None,
                }
            );
        }

        // Test mismatched architectures
        for (target, platform) in &[
            (&target_x86, "win-amd64"),
            (&target_x86, "win-arm64"),
            (&target_x64, "win32"),
            (&target_x64, "win-arm64"),
            (&target_arm64, "win32"),
            (&target_arm64, "win-amd64"),
        ] {
            assert_eq!(
                PythonInterpreter::from_metadata_message(
                    "python3.10",
                    target,
                    &bridge,
                    message(3, 10, platform),
                )
                .unwrap(),
                None
            );
        }

        // Test edge case with unknown platform (should not match any specific architecture, build anyway)
        assert_eq!(
            PythonInterpreter::from_metadata_message(
                "python3.10",
                &target_x64,
                &bridge,
                message(3, 10, "unknown-platform"),
            )
            .unwrap()
            .unwrap(),
            PythonInterpreter {
                config: InterpreterConfig {
                    major: 3,
                    minor: 10,
                    interpreter_kind: InterpreterKind::CPython,
                    abiflags: "".to_string(),
                    ext_suffix: ".pyd".to_string(),
                    pointer_width: None,
                    gil_disabled: false,
                },
                executable: PathBuf::from("python3.10"),
                platform: Some("unknown_platform".to_string()),
                runnable: true,
                implementation_name: "CPython".to_string(),
                soabi: None,
            }
        );
    }

    #[test]
    fn test_interpreter_from_metadata_windows_314() {
        // Test that Python 3.14+ on Windows with ABIFLAGS defined works (fixes #2740)
        let target_x64 = Target::from_resolved_target_triple("x86_64-pc-windows-msvc").unwrap();

        let bridge = BridgeModel::PyO3(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version: semver::Version::new(0, 26, 0),
            abi3: None,
            metadata: None,
        });

        // Python 3.14 with ABIFLAGS="" (standard build)
        let message_314 = InterpreterMetadataMessage {
            major: 3,
            minor: 14,
            interpreter: "cpython".to_string(),
            implementation_name: "CPython".to_string(),
            abiflags: Some("".to_string()),
            ext_suffix: Some(".cp314-win_amd64.pyd".to_string()),
            platform: "win-amd64".to_string(),
            executable: None,
            soabi: None,
            gil_disabled: false,
            system: "windows".to_string(),
        };
        let interp = PythonInterpreter::from_metadata_message(
            "python3.14",
            &target_x64,
            &bridge,
            message_314,
        )
        .unwrap()
        .unwrap();
        assert_eq!(interp.major, 3);
        assert_eq!(interp.minor, 14);
        assert_eq!(interp.abiflags, "");

        // Python 3.14t free-threaded with ABIFLAGS="t"
        let message_314t = InterpreterMetadataMessage {
            major: 3,
            minor: 14,
            interpreter: "cpython".to_string(),
            implementation_name: "CPython".to_string(),
            abiflags: Some("t".to_string()),
            ext_suffix: Some(".cp314t-win_amd64.pyd".to_string()),
            platform: "win-amd64".to_string(),
            executable: None,
            soabi: None,
            gil_disabled: true,
            system: "windows".to_string(),
        };
        let interp = PythonInterpreter::from_metadata_message(
            "python3.14t",
            &target_x64,
            &bridge,
            message_314t,
        )
        .unwrap()
        .unwrap();
        assert_eq!(interp.major, 3);
        assert_eq!(interp.minor, 14);
        assert_eq!(interp.abiflags, "t");
        assert!(interp.gil_disabled);
    }

    #[test]
    fn test_check_executables_single_missing() {
        let target = Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap();
        let bridge = BridgeModel::Bin(None);
        let executables = vec![PathBuf::from("nonexistent-python-1")];

        let result = PythonInterpreter::check_executables(&executables, &target, &bridge);
        let err_msg = result.unwrap_err().to_string();
        assert_snapshot!(err_msg, @"The following Python interpreters could not be found: `nonexistent-python-1`");
    }

    #[test]
    fn test_check_executables_multiple_missing() {
        let target = Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap();
        let bridge = BridgeModel::Bin(None);
        let executables = vec![
            PathBuf::from("nonexistent-python-1"),
            PathBuf::from("nonexistent-python-2"),
        ];

        let result = PythonInterpreter::check_executables(&executables, &target, &bridge);
        let err_msg = result.unwrap_err().to_string();
        assert_snapshot!(err_msg, @"The following Python interpreters could not be found: `nonexistent-python-1`, `nonexistent-python-2`");
    }
}
