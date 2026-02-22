//! Centralized Python interpreter resolution.
//!
//! This module consolidates the various interpreter discovery, validation,
//! deduplication, and filtering paths that were previously scattered across
//! `build_options.rs` and `python_interpreter/mod.rs`.

use super::{InterpreterConfig, InterpreterKind, PythonInterpreter};
use crate::cross_compile::{
    find_build_details, find_sysconfigdata, parse_build_details_json_file, parse_sysconfigdata,
};
use crate::{BridgeModel, Target};
use anyhow::{Context, Result, bail, format_err};
use pep440_rs::VersionSpecifiers;
use std::env;
use std::path::{Path, PathBuf};
use tracing::debug;

use crate::bridge::{Abi3Version, PyO3};

/// Encapsulates all inputs and logic for resolving Python interpreters.
///
/// Instead of 6+ overlapping free functions, this struct provides a single
/// `resolve()` entry point that handles all combinations of:
/// - abi3 vs non-abi3
/// - cross-compile vs native
/// - Windows vs Unix
/// - user-specified interpreters vs auto-discovery
pub struct InterpreterResolver<'a> {
    pub(crate) target: &'a Target,
    pub(crate) bridge: &'a BridgeModel,
    pub(crate) requires_python: Option<&'a VersionSpecifiers>,
    pub(crate) user_interpreters: &'a [PathBuf],
    pub(crate) find_interpreter: bool,
    pub(crate) generate_import_lib: bool,
}

impl<'a> InterpreterResolver<'a> {
    /// Main entry point: resolve the list of Python interpreters to build for.
    pub fn resolve(&self) -> Result<Vec<PythonInterpreter>> {
        match self.bridge {
            BridgeModel::Cffi => self.resolve_single("cffi").map(|i| vec![i]),
            BridgeModel::Bin(None) | BridgeModel::UniFfi => Ok(vec![]),
            BridgeModel::PyO3(pyo3) | BridgeModel::Bin(Some(pyo3)) => self.resolve_pyo3(pyo3),
        }
    }

    /// Resolve interpreters for pyo3/pyo3-ffi bindings (including Bin(Some(pyo3))).
    fn resolve_pyo3(&self, pyo3: &PyO3) -> Result<Vec<PythonInterpreter>> {
        match &pyo3.abi3 {
            None | Some(Abi3Version::CurrentPython) => self.resolve_pyo3_no_fixed_abi3(),
            Some(Abi3Version::Version(major, minor)) => self.resolve_pyo3_abi3(*major, *minor),
        }
    }

    /// Resolve for pyo3 without a fixed abi3 version (non-abi3 or CurrentPython abi3).
    fn resolve_pyo3_no_fixed_abi3(&self) -> Result<Vec<PythonInterpreter>> {
        // Check for PYO3_CONFIG_FILE override
        if let Some(config_file) = env::var_os("PYO3_CONFIG_FILE") {
            let config = InterpreterConfig::from_pyo3_config(config_file.as_ref(), self.target)
                .context("Invalid PYO3_CONFIG_FILE")?;
            return Ok(vec![PythonInterpreter::from_config(config)]);
        }

        // Cross-compilation with PYO3_CROSS_LIB_DIR
        if self.target.cross_compiling() {
            if let Some(cross_lib_dir) = env::var_os("PYO3_CROSS_LIB_DIR") {
                return self.resolve_cross_compile(cross_lib_dir.as_ref());
            }

            // Cross-compiling without PYO3_CROSS_LIB_DIR: use sysconfig
            return self.resolve_cross_no_lib_dir();
        }

        // Native build: discover interpreters
        let interpreters = self.discover_interpreters()?;
        self.print_found(&interpreters);
        Ok(interpreters)
    }

    /// Resolve for pyo3 with a fixed abi3 version (e.g. abi3-py38).
    fn resolve_pyo3_abi3(&self, major: u8, minor: u8) -> Result<Vec<PythonInterpreter>> {
        // Try to find real interpreters on the host first
        let found = self.try_find_host_interpreters();

        // Apply fallback/sysconfig strategies
        let found = match found {
            Ok(interps) => interps,
            Err(err) => {
                // Fallback: try sysconfig-derived interpreters
                if self.target.is_windows() && !self.generate_import_lib {
                    return Err(err.context(
                        "Need a Python interpreter to compile for Windows without \
                         PyO3's `generate-import-lib` feature",
                    ));
                }
                let sysconfig_interps = find_interpreter_in_sysconfig(
                    self.bridge,
                    self.user_interpreters,
                    self.target,
                    self.requires_python,
                )
                .unwrap_or_default();
                if sysconfig_interps.is_empty() && !self.user_interpreters.is_empty() {
                    return Err(err);
                }
                sysconfig_interps
            }
        };

        // For abi3 builds, apply smart interpreter selection:
        // - Prefer non-free-threaded CPython (which supports abi3) over free-threaded
        // - Only include non-abi3-capable interpreters (PyPy, free-threaded CPython)
        //   if explicitly requested by the user via -i
        let found = self.filter_for_abi3(found);

        // Platform-specific abi3 handling
        if self.target.is_windows() {
            return self.resolve_abi3_windows(found, major, minor);
        }

        if self.target.cross_compiling() {
            return self.resolve_abi3_cross_compile(found);
        }

        if !found.is_empty() {
            self.print_found(&found);
            Ok(found)
        } else if self.user_interpreters.is_empty() {
            eprintln!("🐍 Not using a specific python interpreter");
            Ok(vec![
                self.make_fake_interpreter(major as usize, minor as usize),
            ])
        } else {
            bail!("Failed to find any python interpreter");
        }
    }

    /// Check if any user-specified interpreter looks like PyPy.
    fn user_requested_pypy(&self) -> bool {
        !self.user_interpreters.is_empty()
            && self.user_interpreters.iter().any(|p| {
                let s = p.display().to_string();
                s.contains("pypy")
            })
    }

    /// Check if any user-specified interpreter looks like free-threaded Python.
    fn user_requested_free_threaded(&self) -> bool {
        !self.user_interpreters.is_empty()
            && self.user_interpreters.iter().any(|p| {
                let s = p.display().to_string();
                s.ends_with('t') && s.chars().rev().nth(1).is_some_and(|c| c.is_ascii_digit())
            })
    }

    /// Filter interpreters for abi3 builds.
    ///
    /// When building abi3 wheels, we prefer interpreters that support the stable API.
    /// Non-abi3-capable interpreters (PyPy, free-threaded CPython) are only included
    /// if explicitly requested by the user via `-i`.
    ///
    /// This fixes:
    /// - #2772: free-threaded interpreter chosen over non-free-threaded for abi3
    /// - #2852: unexpected PyPy wheel generated for abi3 cross-compile
    /// - #2607: PyPy from `-i` is now honored (not silently dropped)
    fn filter_for_abi3(&self, interpreters: Vec<PythonInterpreter>) -> Vec<PythonInterpreter> {
        if interpreters.is_empty() {
            return interpreters;
        }

        let user_requested_pypy = self.user_requested_pypy();
        let user_requested_free_threaded = self.user_requested_free_threaded();

        let (abi3_capable, non_abi3): (Vec<_>, Vec<_>) = interpreters
            .into_iter()
            .partition(|interp| interp.has_stable_api());

        let mut result = abi3_capable;

        // Only include non-abi3-capable interpreters if explicitly requested
        for interp in non_abi3 {
            let excluded = match interp.interpreter_kind {
                InterpreterKind::PyPy => !user_requested_pypy,
                InterpreterKind::CPython if interp.gil_disabled => !user_requested_free_threaded,
                _ => false,
            };
            if !excluded {
                result.push(interp);
            }
        }

        result
    }

    /// Handle abi3 on Windows.
    fn resolve_abi3_windows(
        &self,
        found: Vec<PythonInterpreter>,
        major: u8,
        minor: u8,
    ) -> Result<Vec<PythonInterpreter>> {
        if env::var_os("PYO3_CROSS_LIB_DIR").is_some() {
            eprintln!("⚠️  Cross-compiling is poorly supported");
            return Ok(vec![
                self.make_fake_interpreter(major as usize, minor as usize),
            ]);
        }

        if let Some(config_file) = env::var_os("PYO3_CONFIG_FILE") {
            let config = InterpreterConfig::from_pyo3_config(config_file.as_ref(), self.target)
                .context("Invalid PYO3_CONFIG_FILE")?;
            return Ok(vec![PythonInterpreter::from_config(config)]);
        }

        if self.generate_import_lib {
            eprintln!(
                "🐍 Not using a specific python interpreter \
                 (automatically generating windows import library)"
            );
            let mut result = found;
            if result.is_empty() {
                result.push(self.make_fake_interpreter(major as usize, minor as usize));
            }
            return Ok(result);
        }

        if found.is_empty() {
            bail!("Failed to find any python interpreter");
        }
        Ok(found)
    }

    /// Handle abi3 cross-compilation.
    fn resolve_abi3_cross_compile(
        &self,
        found: Vec<PythonInterpreter>,
    ) -> Result<Vec<PythonInterpreter>> {
        let mut interps = Vec::with_capacity(found.len());
        let mut pypys = Vec::new();
        for interp in found {
            if interp.interpreter_kind.is_pypy() {
                // Only include PyPy in cross-compile abi3 if explicitly requested (#2852)
                if self.user_requested_pypy() {
                    pypys.push(PathBuf::from(format!(
                        "pypy{}.{}",
                        interp.major, interp.minor
                    )));
                }
            } else {
                interps.push(interp);
            }
        }
        // Cross-compiling to PyPy with abi3: can't use host pypy, use sysconfig
        if !pypys.is_empty() {
            interps.extend(find_interpreter_in_sysconfig(
                self.bridge,
                &pypys,
                self.target,
                self.requires_python,
            )?);
        }
        if interps.is_empty() {
            bail!("Failed to find any python interpreter");
        }
        Ok(interps)
    }

    /// Cross-compile with PYO3_CROSS_LIB_DIR set.
    fn resolve_cross_compile(&self, cross_lib_path: &Path) -> Result<Vec<PythonInterpreter>> {
        if let Some(build_details_path) = find_build_details(cross_lib_path) {
            eprintln!("🐍 Using build-details.json for cross-compiling preparation");
            let config = parse_build_details_json_file(&build_details_path)?;
            let host_python = self.find_host_python()?;
            self.set_pyo3_env(&host_python);
            let soabi = soabi_from_ext_suffix(&config.ext_suffix);
            let implementation_name = config.interpreter_kind.to_string().to_ascii_lowercase();
            Ok(vec![PythonInterpreter {
                config,
                executable: PathBuf::new(),
                platform: None,
                runnable: false,
                implementation_name,
                soabi,
            }])
        } else {
            let host_python = self.find_host_python()?;
            eprintln!("🐍 Using host {host_python} for cross-compiling preparation");
            self.set_pyo3_env(&host_python);
            let sysconfig_path = find_sysconfigdata(cross_lib_path, self.target)?;
            let sysconfig_data = parse_sysconfigdata(&host_python, sysconfig_path)?;
            self.interpreter_from_sysconfigdata(&sysconfig_data)
        }
    }

    /// Cross-compile without PYO3_CROSS_LIB_DIR.
    fn resolve_cross_no_lib_dir(&self) -> Result<Vec<PythonInterpreter>> {
        if self.user_interpreters.is_empty() && !self.find_interpreter {
            bail!(
                "Couldn't find any python interpreters. \
                 Please specify at least one with -i"
            );
        }

        // Check if user-specified interpreters are valid file paths
        for interp in self.user_interpreters {
            if interp.components().count() > 1
                && PythonInterpreter::check_executable(interp, self.target, self.bridge)?.is_none()
            {
                bail!("{} is not a valid python interpreter", interp.display());
            }
        }

        let interpreters = find_interpreter_in_sysconfig(
            self.bridge,
            self.user_interpreters,
            self.target,
            self.requires_python,
        )?;
        if interpreters.is_empty() {
            bail!(
                "Couldn't find any python interpreters from '{}'. \
                 Please check that both major and minor python version \
                 have been specified in -i/--interpreter.",
                self.user_interpreters
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(interpreters)
    }

    /// Find a host Python interpreter for cross-compilation.
    fn find_host_python(&self) -> Result<PythonInterpreter> {
        let host_interps = find_interpreter_in_host(
            self.bridge,
            self.user_interpreters,
            self.target,
            self.requires_python,
        )?;
        Ok(host_interps
            .into_iter()
            .next()
            .expect("find_interpreter_in_host returned empty"))
    }

    /// Set PYO3_PYTHON and PYTHON_SYS_EXECUTABLE environment variables.
    fn set_pyo3_env(&self, host_python: &PythonInterpreter) {
        unsafe {
            env::set_var("PYO3_PYTHON", &host_python.executable);
            env::set_var("PYTHON_SYS_EXECUTABLE", &host_python.executable);
        }
    }

    /// Build a PythonInterpreter from sysconfigdata.
    fn interpreter_from_sysconfigdata(
        &self,
        data: &std::collections::HashMap<String, String>,
    ) -> Result<Vec<PythonInterpreter>> {
        let major = data
            .get("version_major")
            .context("version_major is not defined")?
            .parse::<usize>()
            .context("Could not parse value of version_major")?;
        let minor = data
            .get("version_minor")
            .context("version_minor is not defined")?
            .parse::<usize>()
            .context("Could not parse value of version_minor")?;
        let abiflags = data
            .get("ABIFLAGS")
            .map(ToString::to_string)
            .unwrap_or_default();
        let gil_disabled = data
            .get("Py_GIL_DISABLED")
            .map(|x| x == "1")
            .unwrap_or_default();
        let ext_suffix = data
            .get("EXT_SUFFIX")
            .context("sysconfig didn't define an `EXT_SUFFIX` ಠ_ಠ")?;
        let soabi = data.get("SOABI");
        let interpreter_kind = soabi
            .and_then(|tag| {
                if tag.starts_with("pypy") {
                    Some(InterpreterKind::PyPy)
                } else if tag.starts_with("cpython") {
                    Some(InterpreterKind::CPython)
                } else if tag.starts_with("graalpy") {
                    Some(InterpreterKind::GraalPy)
                } else {
                    None
                }
            })
            .context("unsupported Python interpreter")?;
        Ok(vec![PythonInterpreter {
            config: InterpreterConfig {
                major,
                minor,
                interpreter_kind,
                abiflags,
                ext_suffix: ext_suffix.to_string(),
                pointer_width: None,
                gil_disabled,
            },
            executable: PathBuf::new(),
            platform: None,
            runnable: false,
            implementation_name: interpreter_kind.to_string().to_ascii_lowercase(),
            soabi: soabi.cloned(),
        }])
    }

    /// Discover interpreters: either user-specified + fallback, or auto-discovery.
    fn discover_interpreters(&self) -> Result<Vec<PythonInterpreter>> {
        if self.find_interpreter {
            // --find-interpreter: auto-discover all
            PythonInterpreter::find_all(self.target, self.bridge, self.requires_python)
                .context("Finding python interpreters failed")
        } else if !self.user_interpreters.is_empty() {
            // User specified -i: try host first, sysconfig fallback
            find_interpreter(
                self.bridge,
                self.user_interpreters,
                self.target,
                self.requires_python,
                self.generate_import_lib,
            )
        } else {
            // Default: use PYO3_PYTHON or system python
            let python = self.get_default_python();
            find_interpreter(
                self.bridge,
                &[python],
                self.target,
                self.requires_python,
                self.generate_import_lib,
            )
        }
    }

    /// Try to find host interpreters, returning Err if none found.
    fn try_find_host_interpreters(&self) -> Result<Vec<PythonInterpreter>> {
        find_interpreter_in_host(
            self.bridge,
            self.user_interpreters,
            self.target,
            self.requires_python,
        )
    }

    /// Resolve a single interpreter for cffi or similar.
    fn resolve_single(&self, bridge_name: &str) -> Result<PythonInterpreter> {
        let interp = find_single_python_interpreter(
            self.bridge,
            self.user_interpreters,
            self.target,
            bridge_name,
        )?;
        eprintln!("🐍 Using {interp} to generate the {bridge_name} bindings");
        Ok(interp)
    }

    /// Get the default python executable to use.
    fn get_default_python(&self) -> PathBuf {
        if self.bridge.is_pyo3() {
            std::env::var("PYO3_PYTHON")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| self.target.get_python())
        } else {
            self.target.get_python()
        }
    }

    /// Create a non-runnable placeholder interpreter for abi3 when no real one is found.
    fn make_fake_interpreter(&self, major: usize, minor: usize) -> PythonInterpreter {
        PythonInterpreter {
            config: InterpreterConfig {
                major,
                minor,
                interpreter_kind: InterpreterKind::CPython,
                abiflags: String::new(),
                ext_suffix: if self.target.is_windows() {
                    ".pyd".to_string()
                } else {
                    String::new()
                },
                pointer_width: None,
                gil_disabled: false,
            },
            executable: PathBuf::new(),
            platform: None,
            runnable: false,
            implementation_name: "cpython".to_string(),
            soabi: None,
        }
    }

    /// Print the found interpreters.
    fn print_found(&self, interpreters: &[PythonInterpreter]) {
        if !interpreters.is_empty() {
            let s = interpreters
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!("🐍 Found {s}");
        }
    }
}

// --- Helper functions (kept as module-level for reuse) ---

/// Shared between cffi and pyo3-abi3: find exactly one interpreter.
fn find_single_python_interpreter(
    bridge: &BridgeModel,
    interpreter: &[PathBuf],
    target: &Target,
    bridge_name: &str,
) -> Result<PythonInterpreter> {
    let interpreter_str = interpreter
        .iter()
        .map(|interpreter| format!("`{}`", interpreter.display()))
        .collect::<Vec<_>>()
        .join(", ");
    let err_message = format!("Failed to find a python interpreter from {interpreter_str}");

    let executable = if interpreter.is_empty() {
        target.get_python()
    } else if interpreter.len() == 1 {
        interpreter[0].clone()
    } else {
        bail!(
            "You can only specify one python interpreter for {}",
            bridge_name
        );
    };

    let interpreter = PythonInterpreter::check_executable(executable, target, bridge)
        .context(format_err!(err_message.clone()))?
        .ok_or_else(|| format_err!(err_message))?;
    Ok(interpreter)
}

/// Find python interpreters in the host machine first,
/// fallback to bundled sysconfig if not found.
fn find_interpreter(
    bridge: &BridgeModel,
    interpreter: &[PathBuf],
    target: &Target,
    requires_python: Option<&VersionSpecifiers>,
    generate_import_lib: bool,
) -> Result<Vec<PythonInterpreter>> {
    let mut found_interpreters = Vec::new();
    if !interpreter.is_empty() {
        let mut missing = Vec::new();
        for interp in interpreter {
            match PythonInterpreter::check_executable(interp.clone(), target, bridge)? {
                Some(interp) => found_interpreters.push(interp),
                None => missing.push(interp.clone()),
            }
        }
        if !missing.is_empty() {
            let sysconfig_interps =
                find_interpreter_in_sysconfig(bridge, &missing, target, requires_python)?;

            // Can only use sysconfig-derived interpreter on windows if generating the import lib
            if !sysconfig_interps.is_empty() && target.is_windows() && !generate_import_lib {
                let found = sysconfig_interps
                    .iter()
                    .map(|i| format!("{} {}.{}", i.interpreter_kind, i.major, i.minor))
                    .collect::<Vec<_>>();
                bail!(
                    "Interpreters {found:?} were found in maturin's bundled sysconfig, \
                     but compiling for Windows without an interpreter requires \
                     PyO3's `generate-import-lib` feature"
                );
            }

            found_interpreters.extend(sysconfig_interps);
        }
    } else {
        found_interpreters = PythonInterpreter::find_all(target, bridge, requires_python)
            .context("Finding python interpreters failed")?;
    }

    if found_interpreters.is_empty() {
        if interpreter.is_empty() {
            if let Some(requires_python) = requires_python {
                bail!(
                    "Couldn't find any python interpreters with version {}. \
                     Please specify at least one with -i",
                    requires_python
                );
            } else {
                bail!(
                    "Couldn't find any python interpreters. \
                     Please specify at least one with -i"
                );
            }
        } else {
            let interps_str = interpreter
                .iter()
                .map(|path| format!("'{}'", path.display()))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "Couldn't find any python interpreters from {}.",
                interps_str
            );
        }
    }
    Ok(found_interpreters)
}

/// Find python interpreters in the host machine.
fn find_interpreter_in_host(
    bridge: &BridgeModel,
    interpreter: &[PathBuf],
    target: &Target,
    requires_python: Option<&VersionSpecifiers>,
) -> Result<Vec<PythonInterpreter>> {
    let interpreters = if !interpreter.is_empty() {
        PythonInterpreter::check_executables(interpreter, target, bridge)?
    } else {
        PythonInterpreter::find_all(target, bridge, requires_python)
            .context("Finding python interpreters failed")?
    };

    if interpreters.is_empty() {
        if let Some(requires_python) = requires_python {
            bail!(
                "Couldn't find any python interpreters with {}. \
                 Please specify at least one with -i",
                requires_python
            );
        } else {
            bail!(
                "Couldn't find any python interpreters. \
                 Please specify at least one with -i"
            );
        }
    }
    Ok(interpreters)
}

/// Find python interpreters in the bundled sysconfig.
fn find_interpreter_in_sysconfig(
    bridge: &BridgeModel,
    interpreter: &[PathBuf],
    target: &Target,
    requires_python: Option<&VersionSpecifiers>,
) -> Result<Vec<PythonInterpreter>> {
    if interpreter.is_empty() {
        return Ok(PythonInterpreter::find_by_target(
            target,
            requires_python,
            Some(bridge),
        ));
    }
    let mut interpreters = Vec::new();
    for interp in interpreter {
        let python = interp.display().to_string();
        let (python_impl, python_ver, abiflags) = if let Some(ver) = python.strip_prefix("pypy") {
            (
                InterpreterKind::PyPy,
                ver.strip_prefix('-').unwrap_or(ver),
                "",
            )
        } else if let Some(ver) = python.strip_prefix("graalpy") {
            (
                InterpreterKind::GraalPy,
                ver.strip_prefix('-').unwrap_or(ver),
                "",
            )
        } else if let Some(ver) = python.strip_prefix("python") {
            let (ver, abiflags) = maybe_free_threaded(ver.strip_prefix('-').unwrap_or(ver));
            (InterpreterKind::CPython, ver, abiflags)
        } else if python
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            let (ver, abiflags) = maybe_free_threaded(&python);
            (InterpreterKind::CPython, ver, abiflags)
        } else if std::path::Path::new(&python).is_file() {
            bail!(
                "Python interpreter should be a kind of interpreter \
                 (e.g. 'python3.14' or 'pypy3.11') when cross-compiling, \
                 got path to interpreter: {}",
                python
            );
        } else {
            bail!(
                "Unsupported Python interpreter for cross-compilation: {}; \
                 supported interpreters are pypy, graalpy, and python (cpython)",
                python
            );
        };
        if python_ver.is_empty() {
            continue;
        }
        let (ver_major, ver_minor) = python_ver
            .split_once('.')
            .context("Invalid python interpreter version")?;
        let ver_major = ver_major.parse::<usize>().with_context(|| {
            format!("Invalid python interpreter major version '{ver_major}', expect a digit")
        })?;
        let ver_minor = ver_minor.parse::<usize>().with_context(|| {
            format!("Invalid python interpreter minor version '{ver_minor}', expect a digit")
        })?;

        if (ver_major, ver_minor) < (3, 13) && abiflags == "t" {
            bail!("Free-threaded Python interpreter is only supported on 3.13 and later.");
        }

        let sysconfig =
            InterpreterConfig::lookup_one(target, python_impl, (ver_major, ver_minor), abiflags)
                .with_context(|| {
                    format!(
                        "Failed to find a {python_impl} {ver_major}.{ver_minor} \
                     interpreter in known sysconfig"
                    )
                })?;
        debug!(
            "Found {} {}.{}{} in bundled sysconfig",
            sysconfig.interpreter_kind, sysconfig.major, sysconfig.minor, sysconfig.abiflags
        );
        interpreters.push(PythonInterpreter::from_config(sysconfig.clone()));
    }
    Ok(interpreters)
}

/// Derive SOABI from an extension suffix.
///
/// For example, `.cpython-314-x86_64-linux-gnu.so` becomes
/// `cpython-314-x86_64-linux-gnu`.
fn soabi_from_ext_suffix(ext_suffix: &str) -> Option<String> {
    let s = ext_suffix.strip_prefix('.')?;
    let s = s
        .strip_suffix(".so")
        .or_else(|| s.strip_suffix(".pyd"))
        .unwrap_or(s);
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn maybe_free_threaded(python_ver: &str) -> (&str, &str) {
    if let Some(ver) = python_ver.strip_suffix('t') {
        (ver, "t")
    } else {
        (python_ver, "")
    }
}
