pub use self::config::InterpreterConfig;
use crate::auditwheel::PlatformTag;
use crate::{BridgeModel, BuildContext, Target};
use anyhow::{bail, format_err, Context, Result};
use pep440_rs::{Version, VersionSpecifiers};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;
use std::io::{self, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::{self, FromStr};
use tracing::debug;

mod config;

/// This snippets will give us information about the python interpreter's
/// version and abi as json through stdout
const GET_INTERPRETER_METADATA: &str = include_str!("get_interpreter_metadata.py");
pub const MINIMUM_PYTHON_MINOR: usize = 7;
/// Be liberal here to include preview versions
pub const MAXIMUM_PYTHON_MINOR: usize = 12;
pub const MAXIMUM_PYPY_MINOR: usize = 10;

/// Identifies conditions where we do not want to build wheels
fn windows_interpreter_no_build(
    major: usize,
    minor: usize,
    target_width: usize,
    pointer_width: usize,
    min_python_minor: usize,
    requires_python: Option<&VersionSpecifiers>,
) -> bool {
    // Only python 3 with supported major versions
    if major != 3 || minor < min_python_minor {
        return true;
    }

    // From requires-python in pyproject.toml
    if let Some(requires_python) = requires_python {
        if !requires_python.contains(&Version::from_release(vec![major, minor])) {
            return true;
        }
    }

    // There can be 32-bit installations on a 64-bit machine, but we can't link
    // those for 64-bit targets
    if pointer_width != target_width {
        eprintln!(
            "👽 {major}.{minor} is installed as {pointer_width}-bit, while the target is {target_width}-bit. Skipping."
        );
        return true;
    }
    false
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
fn find_all_windows(
    target: &Target,
    min_python_minor: usize,
    requires_python: Option<&VersionSpecifiers>,
) -> Result<Vec<String>> {
    let code = "import sys; print(sys.executable or '')";
    let mut interpreter = vec![];
    let mut versions_found = HashSet::new();

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
                if !versions_found.contains(&(major, minor)) {
                    let pointer_width = capture
                        .get(5)
                        .map(|m| m.as_str())
                        .filter(|m| !m.is_empty())
                        .unwrap_or("64")
                        .parse::<usize>()
                        .context("Expected a digit for pointer width")?;

                    if windows_interpreter_no_build(
                        major,
                        minor,
                        target.pointer_width(),
                        pointer_width,
                        min_python_minor,
                        requires_python,
                    ) {
                        continue;
                    }

                    let executable = capture.get(6).unwrap().as_str();
                    let output = Command::new(executable).args(["-c", code]).output();
                    let output = match output {
                        Ok(output) => output,
                        Err(err) => {
                            eprintln!(
                                "⚠️  Warning: failed to determine the path to python for `{executable}`: {err}"
                            );
                            continue;
                        }
                    };
                    let path = str::from_utf8(&output.stdout).unwrap().trim();
                    if !output.status.success() || path.trim().is_empty() {
                        eprintln!(
                            "⚠️  Warning: couldn't determine the path to python for `{executable}`"
                        );
                        continue;
                    }
                    interpreter.push(path.to_string());
                    versions_found.insert((major, minor));
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
            if let Some(python_info) = windows_python_info(&executable)? {
                if windows_interpreter_no_build(
                    python_info.major,
                    python_info.minor,
                    target.pointer_width(),
                    python_info.pointer_width.unwrap(),
                    min_python_minor,
                    requires_python,
                ) {
                    continue;
                }
                interpreter.push(String::from(executable.to_str().unwrap()));
                versions_found.insert((python_info.major, python_info.minor));
            }
        }
    }

    // Fallback to pythonX.Y for Microsoft Store versions
    for minor in min_python_minor..=MAXIMUM_PYTHON_MINOR {
        if !versions_found.contains(&(3, minor)) {
            let executable = format!("python3.{minor}.exe");
            if let Some(python_info) = windows_python_info(Path::new(&executable))? {
                if windows_interpreter_no_build(
                    python_info.major,
                    python_info.minor,
                    target.pointer_width(),
                    python_info.pointer_width.unwrap(),
                    min_python_minor,
                    requires_python,
                ) {
                    continue;
                }
                interpreter.push(executable);
                versions_found.insert((3, minor));
            }
        }
    }

    if interpreter.is_empty() {
        bail!(
            "Could not find any interpreters, are you sure you have python installed on your PATH?"
        );
    };
    Ok(interpreter)
}

fn windows_python_info(executable: &Path) -> Result<Option<InterpreterConfig>> {
    let python_info = Command::new(executable)
        .arg("-c")
        .arg("import sys; print(sys.version)")
        .output();

    let python_info = match python_info {
        Ok(python_info) => python_info,
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound {
                // python executable not found
                return Ok(None);
            } else {
                bail!(
                    "Error getting Python version info from {}",
                    executable.display()
                );
            }
        }
    };

    let version_info = str::from_utf8(&python_info.stdout).unwrap();
    let expr = Regex::new(r"(\d).(\d).(\d+)").unwrap();
    if let Some(capture) = expr.captures(version_info) {
        let major = capture.get(1).unwrap().as_str().parse::<usize>().unwrap();
        let minor = capture.get(2).unwrap().as_str().parse::<usize>().unwrap();
        let pointer_width = if version_info.contains("64 bit (AMD64)") {
            64
        } else {
            32
        };
        Ok(Some(InterpreterConfig {
            major,
            minor,
            interpreter_kind: InterpreterKind::CPython,
            abiflags: String::new(),
            ext_suffix: String::new(),
            pointer_width: Some(pointer_width),
        }))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lower")]
pub enum InterpreterKind {
    CPython,
    PyPy,
    GraalPy,
}

impl InterpreterKind {
    /// Is this a CPython interpreter?
    pub fn is_cpython(&self) -> bool {
        matches!(self, InterpreterKind::CPython)
    }

    /// Is this a PyPy interpreter?
    pub fn is_pypy(&self) -> bool {
        matches!(self, InterpreterKind::PyPy)
    }

    /// Is this a GraalPy interpreter?
    pub fn is_graalpy(&self) -> bool {
        matches!(self, InterpreterKind::GraalPy)
    }
}

impl fmt::Display for InterpreterKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            InterpreterKind::CPython => write!(f, "CPython"),
            InterpreterKind::PyPy => write!(f, "PyPy"),
            InterpreterKind::GraalPy => write!(f, "GraalPy"),
        }
    }
}

impl FromStr for InterpreterKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "cpython" => Ok(InterpreterKind::CPython),
            "pypy" => Ok(InterpreterKind::PyPy),
            "graalvm" | "graalpy" => Ok(InterpreterKind::GraalPy),
            unknown => Err(format!("Unknown interpreter kind '{unknown}'")),
        }
    }
}

/// The output format of [GET_INTERPRETER_METADATA]
#[derive(Deserialize)]
struct InterpreterMetadataMessage {
    implementation_name: String,
    executable: Option<String>,
    major: usize,
    minor: usize,
    abiflags: Option<String>,
    interpreter: String,
    ext_suffix: Option<String>,
    // comes from `sysconfig.get_platform()`
    platform: String,
    // comes from `platform.system()`
    system: String,
    soabi: Option<String>,
}

/// The location and version of an interpreter
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PythonInterpreter {
    /// Python's sysconfig
    /// Python's major version
    pub config: InterpreterConfig,
    /// Path to the python interpreter, e.g. /usr/bin/python3.6
    ///
    /// Just the name of the binary in PATH does also work, e.g. `python3.5`
    pub executable: PathBuf,
    /// Comes from `sysconfig.get_platform()`
    ///
    /// Note that this can be `None` when cross compiling
    pub platform: Option<String>,
    /// Is this interpreter runnable
    ///
    /// When cross compile the target interpreter isn't runnable,
    /// and it's `executable` is empty
    pub runnable: bool,
    /// Comes from `sys.platform.name`
    pub implementation_name: String,
    /// Comes from sysconfig var `SOABI`
    pub soabi: Option<String>,
}

impl Deref for PythonInterpreter {
    type Target = InterpreterConfig;

    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

/// Returns the abiflags that are assembled through the message, with some
/// additional sanity checks.
///
/// The rules are as follows:
///  - python 3 + Unix: Use ABIFLAGS
///  - python 3 + Windows: No ABIFLAGS, return an empty string
fn fun_with_abiflags(
    message: &InterpreterMetadataMessage,
    target: &Target,
    bridge: &BridgeModel,
) -> Result<String> {
    if bridge != &BridgeModel::Cffi
        && target.get_python_os() != message.system
        && !target.cross_compiling()
    {
        bail!(
            "platform.system() in python, {}, and the rust target, {:?}, don't match ಠ_ಠ",
            message.system,
            target,
        )
    }

    if message.major != 3 || message.minor < 7 {
        bail!(
            "Only python >= 3.7 is supported, while you're using python {}.{}",
            message.major,
            message.minor
        );
    }

    if message.interpreter == "pypy" || message.interpreter == "graalvm" {
        // pypy and graalpy do not specify abi flags
        Ok("".to_string())
    } else if message.system == "windows" {
        if matches!(message.abiflags.as_deref(), Some("") | None) {
            Ok("".to_string())
        } else {
            bail!("A python 3 interpreter on windows does not define abiflags in its sysconfig ಠ_ಠ")
        }
    } else if let Some(ref abiflags) = message.abiflags {
        if message.minor >= 8 {
            // for 3.8, "builds with and without pymalloc are ABI compatible" and the flag dropped
            Ok(abiflags.to_string())
        } else if (abiflags != "m") && (abiflags != "dm") {
            bail!("A python 3 interpreter on linux or mac os must have 'm' or 'dm' as abiflags ಠ_ಠ")
        } else {
            Ok(abiflags.to_string())
        }
    } else {
        bail!("A python 3 interpreter on linux or mac os must define abiflags in its sysconfig ಠ_ಠ")
    }
}

impl PythonInterpreter {
    /// Does this interpreter have PEP 384 stable api aka. abi3 support?
    pub fn has_stable_api(&self) -> bool {
        if self.implementation_name.parse::<InterpreterKind>().is_err() {
            false
        } else {
            match self.interpreter_kind {
                InterpreterKind::CPython => true,
                InterpreterKind::PyPy | InterpreterKind::GraalPy => false,
            }
        }
    }

    /// Returns the supported python environment in the PEP 425 format used for the wheel filename:
    /// {python tag}-{abi tag}-{platform tag}
    ///
    /// Don't ask me why or how, this is just what setuptools uses so I'm also going to use
    ///
    /// If abi3 is true, cpython wheels use the generic abi3 with the given version as minimum
    pub fn get_tag(&self, context: &BuildContext, platform_tags: &[PlatformTag]) -> Result<String> {
        // Restrict `sysconfig.get_platform()` usage to Windows and non-portable Linux only for now
        // so we don't need to deal with macOS deployment target
        let target = &context.target;
        let use_sysconfig_platform = target.is_windows()
            || (target.is_linux() && platform_tags.iter().any(|tag| !tag.is_portable()))
            || target.is_illumos();
        let platform = if use_sysconfig_platform {
            if let Some(platform) = self.platform.clone() {
                platform
            } else {
                context.get_platform_tag(platform_tags)?
            }
        } else {
            context.get_platform_tag(platform_tags)?
        };
        let tag = if self.implementation_name.parse::<InterpreterKind>().is_err() {
            // Use generic tags when `sys.implementation.name` != `platform.python_implementation()`, for example Pyston
            // See also https://github.com/pypa/packaging/blob/0031046f7fad649580bc3127d1cef9157da0dd79/packaging/tags.py#L234-L261
            format!(
                "{interpreter}{major}{minor}-{soabi}-{platform}",
                interpreter = self.implementation_name,
                major = self.major,
                minor = self.minor,
                soabi = self
                    .soabi
                    .as_deref()
                    .unwrap_or("none")
                    .replace(['-', '.'], "_"),
                platform = platform
            )
        } else {
            match self.interpreter_kind {
                InterpreterKind::CPython => {
                    if target.is_unix() {
                        format!(
                            "cp{major}{minor}-cp{major}{minor}{abiflags}-{platform}",
                            major = self.major,
                            minor = self.minor,
                            abiflags = self.abiflags,
                            platform = platform
                        )
                    } else {
                        // On windows the abiflags are missing, but this seems to work
                        format!(
                            "cp{major}{minor}-none-{platform}",
                            major = self.major,
                            minor = self.minor,
                            platform = platform
                        )
                    }
                }
                InterpreterKind::PyPy => {
                    // pypy uses its version as part of the ABI, e.g.
                    // pypy 3.7 7.3 => numpy-1.20.1-pp37-pypy37_pp73-manylinux2014_x86_64.whl
                    format!(
                        "pp{major}{minor}-{abi_tag}-{platform}",
                        major = self.major,
                        minor = self.minor,
                        abi_tag = calculate_abi_tag(&self.ext_suffix)
                            .expect("PyPy's syconfig didn't define a valid `EXT_SUFFIX` ಠ_ಠ"),
                        platform = platform,
                    )
                }
                InterpreterKind::GraalPy => {
                    // GraalPy like PyPy uses its version as part of the ABI
                    // graalpy 3.10 23.1 => numpy-1.23.5-graalpy310-graalpy231_310_native_manylinux2014_x86_64.whl
                    format!(
                        "graalpy{major}{minor}-{abi_tag}_{platform}",
                        major = self.major,
                        minor = self.minor,
                        abi_tag = calculate_abi_tag(&self.ext_suffix)
                            .expect("GraalPy's syconfig didn't define a valid `EXT_SUFFIX` ಠ_ಠ"),
                        platform = platform,
                    )
                }
            }
        };
        Ok(tag)
    }

    /// Adds the ext_suffix we read from python or know (.pyd/.abi3.so) and adds it to the base name
    ///
    /// For CPython, generate extensions as follows:
    ///
    /// For python 3, there is PEP 3149, but
    /// that is only valid for 3.2 - 3.4. Since only 3.6+ is supported, the
    /// templates are adapted from the (also
    /// incorrect) release notes of CPython 3.5:
    /// https://docs.python.org/3/whatsnew/3.5.html#build-and-c-api-changes
    ///
    /// Examples for 64-bit on CPython 3.6m:
    /// Linux:   foobar.cpython-36m-x86_64-linux-gnu.so
    /// Windows: foobar.cp36-win_amd64.pyd
    /// Mac:     foobar.cpython-36m-darwin.so
    /// FreeBSD: foobar.cpython-36m.so
    ///
    /// For pypy3, we read importlib.machinery.EXTENSION_SUFFIXES[0].
    pub fn get_library_name(&self, base: &str) -> String {
        format!(
            "{base}{ext_suffix}",
            base = base,
            ext_suffix = self.ext_suffix
        )
    }

    /// Is this a debug build of Python for Windows?
    pub fn is_windows_debug(&self) -> bool {
        self.ext_suffix.starts_with("_d.") && self.ext_suffix.ends_with(".pyd")
    }

    /// Checks whether the given command is a python interpreter and returns a
    /// [PythonInterpreter] if that is the case
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
                            cmd.arg("/c")
                                .arg("py")
                                .arg(format!("-{}-{}", ver, target.pointer_width()))
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

        let platform = if message.platform.starts_with("macosx") {
            // We don't use platform from sysconfig on macOS
            None
        } else {
            Some(message.platform.to_lowercase().replace(['-', '.'], "_"))
        };

        let executable = message
            .executable
            .map(PathBuf::from)
            .unwrap_or_else(|| executable.as_ref().to_path_buf());
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
            },
            executable,
            platform,
            runnable: true,
            implementation_name: message.implementation_name,
            soabi: message.soabi,
        }))
    }

    /// Construct a `PythonInterpreter` from a sysconfig and target
    pub fn from_config(config: InterpreterConfig) -> Self {
        let implementation_name = config.interpreter_kind.to_string().to_ascii_lowercase();
        PythonInterpreter {
            config,
            executable: PathBuf::new(),
            platform: None,
            runnable: false,
            implementation_name,
            soabi: None,
        }
    }

    /// Find all available python interpreters for a given target
    pub fn find_by_target(
        target: &Target,
        requires_python: Option<&VersionSpecifiers>,
    ) -> Vec<PythonInterpreter> {
        InterpreterConfig::lookup_target(target)
            .into_iter()
            .filter_map(|config| match requires_python {
                Some(requires_python) => {
                    if requires_python
                        .contains(&Version::from_release(vec![config.major, config.minor]))
                    {
                        Some(Self::from_config(config))
                    } else {
                        None
                    }
                }
                None => Some(Self::from_config(config)),
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
        let min_python_minor = match bridge {
            BridgeModel::Bindings(_, minor) | BridgeModel::Bin(Some((_, minor))) => *minor,
            _ => MINIMUM_PYTHON_MINOR,
        };
        let executables = if target.is_windows() {
            find_all_windows(target, min_python_minor, requires_python)?
        } else {
            let mut executables: Vec<String> = (min_python_minor..=MAXIMUM_PYTHON_MINOR)
                .filter(|minor| {
                    requires_python
                        .map(|requires_python| {
                            requires_python.contains(&Version::from_release(vec![3, *minor]))
                        })
                        .unwrap_or(true)
                })
                .map(|minor| format!("python3.{minor}"))
                .collect();
            // Also try to find PyPy for cffi and pyo3 bindings
            if *bridge == BridgeModel::Cffi
                || bridge.is_bindings("pyo3")
                || bridge.is_bindings("pyo3-ffi")
            {
                executables.extend(
                    (min_python_minor..=MAXIMUM_PYPY_MINOR)
                        .filter(|minor| {
                            requires_python
                                .map(|requires_python| {
                                    requires_python
                                        .contains(&Version::from_release(vec![3, *minor]))
                                })
                                .unwrap_or(true)
                        })
                        .map(|minor| format!("pypy3.{minor}")),
                );
            }
            executables
        };
        let mut available_versions = Vec::new();
        for executable in executables {
            if let Some(version) = PythonInterpreter::check_executable(executable, target, bridge)?
            {
                available_versions.push(version);
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
        for executable in executables {
            if let Some(version) = PythonInterpreter::check_executable(executable, target, bridge)
                .context(format!(
                "{} is not a valid python interpreter",
                executable.display()
            ))? {
                available_versions.push(version);
            } else {
                bail!(
                    "Python interpreter `{}` doesn't exist",
                    executable.display()
                );
            }
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

    /// Whether this Python interpreter support portable manylinux/musllinux wheels
    ///
    /// Returns `true` if we can not decide
    pub fn support_portable_wheels(&self) -> bool {
        if !self.runnable {
            return true;
        }
        let out = Command::new(&self.executable)
            .args([
                "-m",
                "pip",
                "debug",
                "--verbose",
                "--disable-pip-version-check",
            ])
            .output();

        match out {
            Err(_) => true,
            Ok(ok) if !ok.status.success() => true,
            Ok(ok) => {
                if let Ok(stdout) = String::from_utf8(ok.stdout) {
                    stdout.contains("manylinux") || stdout.contains("musllinux")
                } else {
                    true
                }
            }
        }
    }

    /// An opaque string that uniquely identifies this Python interpreter.
    /// Used to trigger rebuilds for `pyo3` when the Python interpreter changes.
    pub fn environment_signature(&self) -> String {
        let pointer_width = self.pointer_width.unwrap_or(64);
        format!(
            "{}-{}.{}-{}bit",
            self.implementation_name, self.major, self.minor, pointer_width
        )
    }

    /// Returns the site-packages directory inside a venv e.g.
    /// {venv_base}/lib/python{x}.{y} on unix or {venv_base}/Lib on window
    pub fn get_venv_site_package(&self, venv_base: impl AsRef<Path>, target: &Target) -> PathBuf {
        if target.is_unix() {
            match self.interpreter_kind {
                InterpreterKind::CPython | InterpreterKind::GraalPy => {
                    let python_dir = format!("python{}.{}", self.major, self.minor);
                    venv_base
                        .as_ref()
                        .join("lib")
                        .join(python_dir)
                        .join("site-packages")
                }
                InterpreterKind::PyPy => venv_base.as_ref().join("site-packages"),
            }
        } else {
            venv_base.as_ref().join("Lib").join("site-packages")
        }
    }
}

impl fmt::Display for PythonInterpreter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.runnable {
            write!(
                f,
                "{} {}.{}{} at {}",
                self.config.interpreter_kind,
                self.config.major,
                self.config.minor,
                self.config.abiflags,
                self.executable.display()
            )
        } else {
            write!(
                f,
                "{} {}.{}{}",
                self.config.interpreter_kind,
                self.config.major,
                self.config.minor,
                self.config.abiflags,
            )
        }
    }
}

/// Calculate the ABI tag from EXT_SUFFIX
fn calculate_abi_tag(ext_suffix: &str) -> Option<String> {
    let parts = ext_suffix.split('.').collect::<Vec<_>>();
    if parts.len() < 3 {
        // CPython3.7 and earlier uses ".pyd" on Windows.
        return None;
    }
    let soabi = parts[1];
    let mut soabi_split = soabi.split('-');
    let abi = if soabi.starts_with("cpython") {
        // non-windows
        format!("cp{}", soabi_split.nth(1).unwrap())
    } else if soabi.starts_with("cp") {
        // windows
        soabi_split.next().unwrap().to_string()
    } else if soabi.starts_with("pypy") {
        soabi_split.take(2).collect::<Vec<_>>().join("-")
    } else if soabi.starts_with("graalpy") {
        soabi_split.take(3).collect::<Vec<_>>().join("-")
    } else if !soabi.is_empty() {
        // pyston, ironpython, others?
        soabi_split.nth(1).unwrap().to_string()
    } else {
        return None;
    };
    let abi_tag = abi.replace(['.', '-', ' '], "_");
    Some(abi_tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_interpreter_by_target() {
        let target =
            Target::from_target_triple(Some("x86_64-unknown-linux-gnu".to_string())).unwrap();
        let pythons = PythonInterpreter::find_by_target(&target, None);
        assert_eq!(pythons.len(), 10);

        let pythons = PythonInterpreter::find_by_target(
            &target,
            Some(&VersionSpecifiers::from_str(">=3.7").unwrap()),
        );
        assert_eq!(pythons.len(), 10);

        let pythons = PythonInterpreter::find_by_target(
            &target,
            Some(&VersionSpecifiers::from_str(">=3.10").unwrap()),
        );
        assert_eq!(pythons.len(), 4);
    }

    #[test]
    fn test_calculate_abi_tag() {
        let cases = vec![
            (".cpython-37m-x86_64-linux-gnu.so", Some("cp37m")),
            (".cpython-310-x86_64-linux-gnu.so", Some("cp310")),
            (".cpython-310-darwin.so", Some("cp310")),
            (".cp310-win_amd64.pyd", Some("cp310")),
            (".cp39-mingw_x86_64.pyd", Some("cp39")),
            (".cpython-312-wasm32-wasi.so", Some("cp312")),
            (".cpython-38.so", Some("cp38")),
            (".pyd", None),
            (".so", None),
            (".pypy38-pp73-x86_64-linux-gnu.so", Some("pypy38_pp73")),
            (
                ".graalpy-38-native-x86_64-darwin.dylib",
                Some("graalpy_38_native"),
            ),
            (".pyston-23-x86_64-linux-gnu.so", Some("23")),
        ];
        for (ext_suffix, expected) in cases {
            assert_eq!(calculate_abi_tag(ext_suffix).as_deref(), expected);
        }
    }
}
