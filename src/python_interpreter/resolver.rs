//! Centralized Python interpreter resolution.
//!
//! This module consolidates the various interpreter discovery, validation,
//! deduplication, and filtering paths that were previously scattered across
//! `build_options.rs` and `python_interpreter/mod.rs`.
//!
//! The resolution follows a unified pipeline:
//!
//! ```text
//! PYO3_CONFIG_FILE check
//!   → Discover candidates (native, cross+lib_dir, cross+sysconfig)
//!     → Filter (abi3 policy)
//!       → Finalize (fallback to placeholder, platform-specific)
//!         → Convert Candidate → PythonInterpreter
//! ```

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

// ---------------------------------------------------------------------------
// Candidate types
// ---------------------------------------------------------------------------

/// How a candidate Python interpreter was discovered.
///
/// Tracks provenance so the pipeline can make informed decisions
/// (e.g. whether to set `PYO3_PYTHON`, whether the interpreter is runnable).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Variants are part of the design for clarity; some used only in tests/future
enum CandidateSource {
    /// A real executable on the host machine.
    Executable,
    /// From `PYO3_CONFIG_FILE`.
    ConfigFile,
    /// From `PYO3_CROSS_LIB_DIR` (build-details.json or sysconfigdata).
    CrossCompileLib,
    /// From maturin's bundled sysconfig data (no real interpreter available).
    Sysconfig,
    /// A non-runnable placeholder for abi3 when no real interpreter was found.
    Placeholder,
}

/// A Python interpreter candidate found during discovery.
///
/// Wraps a [`PythonInterpreter`] with metadata about how it was found,
/// enabling the resolution pipeline to make informed decisions about
/// filtering, environment setup, and fallback strategies.
#[derive(Debug, Clone)]
#[allow(dead_code)] // `source` is part of the design; used for debugging and future decisions
struct Candidate {
    interpreter: PythonInterpreter,
    source: CandidateSource,
}

impl Candidate {
    fn executable(interp: PythonInterpreter) -> Self {
        Self {
            interpreter: interp,
            source: CandidateSource::Executable,
        }
    }

    fn cross_compile_lib(interp: PythonInterpreter) -> Self {
        Self {
            interpreter: interp,
            source: CandidateSource::CrossCompileLib,
        }
    }

    fn sysconfig(interp: PythonInterpreter) -> Self {
        Self {
            interpreter: interp,
            source: CandidateSource::Sysconfig,
        }
    }

    fn placeholder(interp: PythonInterpreter) -> Self {
        Self {
            interpreter: interp,
            source: CandidateSource::Placeholder,
        }
    }

    /// Convert this candidate into a `PythonInterpreter`, discarding source info.
    fn into_interpreter(self) -> PythonInterpreter {
        self.interpreter
    }
}

/// Classify existing `PythonInterpreter`s as candidates based on `runnable`.
fn to_candidates(interps: Vec<PythonInterpreter>) -> Vec<Candidate> {
    interps
        .into_iter()
        .map(|i| {
            if i.runnable {
                Candidate::executable(i)
            } else {
                Candidate::sysconfig(i)
            }
        })
        .collect()
}

/// Result of interpreter discovery, before filtering/finalization.
struct DiscoveryResult {
    candidates: Vec<Candidate>,
    /// Host Python interpreter found during cross-compile discovery.
    /// Used to set `PYO3_PYTHON` and `PYTHON_SYS_EXECUTABLE` for the build.
    host_python: Option<PythonInterpreter>,
}

/// Result of interpreter resolution.
///
/// In addition to the resolved interpreters, this carries the host Python
/// path discovered during cross-compilation. The caller is responsible for
/// setting `PYO3_PYTHON` / `PYTHON_SYS_EXECUTABLE` using this value
/// before invoking cargo.
#[derive(Debug)]
pub struct ResolveResult {
    pub interpreters: Vec<PythonInterpreter>,
    /// Host Python interpreter found during cross-compile discovery.
    /// The caller should set `PYO3_PYTHON` and `PYTHON_SYS_EXECUTABLE` to
    /// this path before building.
    pub host_python: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------------

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
    ///
    /// Returns a [`ResolveResult`] containing the interpreters and, for
    /// cross-compilation, the host Python path that should be used to set
    /// `PYO3_PYTHON`.
    pub fn resolve(&self) -> Result<ResolveResult> {
        match self.bridge {
            BridgeModel::Cffi => self.resolve_single("cffi").map(|i| ResolveResult {
                interpreters: vec![i],
                host_python: None,
            }),
            BridgeModel::Bin(None) | BridgeModel::UniFfi => Ok(ResolveResult {
                interpreters: vec![],
                host_python: None,
            }),
            BridgeModel::PyO3(pyo3) | BridgeModel::Bin(Some(pyo3)) => self.resolve_pyo3(pyo3),
        }
    }

    // -----------------------------------------------------------------------
    // Unified PyO3 pipeline
    // -----------------------------------------------------------------------

    /// Resolve interpreters for pyo3/pyo3-ffi bindings (including Bin(Some(pyo3))).
    ///
    /// Follows a unified pipeline regardless of native/cross, abi3/non-abi3:
    /// 1. Check `PYO3_CONFIG_FILE` (explicit override)
    /// 2. Discover candidates
    /// 3. Set `PYO3_PYTHON` if cross-compiling with a host interpreter
    /// 4. Filter for abi3 (if applicable)
    /// 5. Finalize: apply fallback policies, validate result
    fn resolve_pyo3(&self, pyo3: &PyO3) -> Result<ResolveResult> {
        // Step 1: PYO3_CONFIG_FILE is an explicit override that trumps everything
        if let Some(config_file) = env::var_os("PYO3_CONFIG_FILE") {
            let config = InterpreterConfig::from_pyo3_config(config_file.as_ref(), self.target)
                .context("Invalid PYO3_CONFIG_FILE")?;
            return Ok(ResolveResult {
                interpreters: vec![PythonInterpreter::from_config(config)],
                host_python: None,
            });
        }

        let fixed_abi3 = match &pyo3.abi3 {
            Some(Abi3Version::Version(major, minor)) => Some((*major, *minor)),
            _ => None,
        };

        // Step 2: Discover candidates (unified for native/cross)
        let discovery = self.discover_candidates(fixed_abi3)?;

        // Capture host_python for the caller to set PYO3_PYTHON
        let host_python = discovery.host_python.as_ref().map(|h| h.executable.clone());

        // Step 3-4: Filter and finalize (differs for abi3 vs non-abi3)
        let interpreters = if let Some((major, minor)) = fixed_abi3 {
            let filtered = self.filter_for_abi3(discovery.candidates);
            self.finalize_abi3(filtered, major, minor)?
        } else {
            let interpreters = Self::candidates_to_interpreters(discovery.candidates);
            self.print_found(&interpreters);
            interpreters
        };

        Ok(ResolveResult {
            interpreters,
            host_python,
        })
    }

    // -----------------------------------------------------------------------
    // Discovery: unified entry point
    // -----------------------------------------------------------------------

    /// Discover interpreter candidates based on the build context.
    ///
    /// Source priority:
    /// 1. `PYO3_CROSS_LIB_DIR` (build-details.json or sysconfigdata)
    /// 2. Cross-compile without lib dir: bundled sysconfig (non-abi3)
    /// 3. Native build / abi3 cross without lib dir: real host interpreters
    fn discover_candidates(&self, fixed_abi3: Option<(u8, u8)>) -> Result<DiscoveryResult> {
        // Cross-compilation with PYO3_CROSS_LIB_DIR
        if self.target.cross_compiling()
            && let Some(cross_lib_dir) = env::var_os("PYO3_CROSS_LIB_DIR")
        {
            // Abi3 Windows cross: just return a placeholder (poorly supported)
            if let Some((major, minor)) = fixed_abi3
                && self.target.is_windows()
            {
                eprintln!("⚠️  Cross-compiling is poorly supported");
                return Ok(DiscoveryResult {
                    candidates: vec![Candidate::placeholder(
                        self.make_fake_interpreter(major as usize, minor as usize),
                    )],
                    host_python: None,
                });
            }
            return self.discover_from_cross_lib_dir(cross_lib_dir.as_ref());
        }

        // Cross-compile without lib dir, non-abi3: use bundled sysconfig
        if self.target.cross_compiling() && fixed_abi3.is_none() {
            return self.discover_cross_sysconfig();
        }

        // Native build, or abi3 cross without PYO3_CROSS_LIB_DIR
        // (abi3 cross can use host interpreters since the wheel is version-independent)
        self.discover_native(fixed_abi3)
    }

    // -----------------------------------------------------------------------
    // Discovery: cross-compile with PYO3_CROSS_LIB_DIR
    // -----------------------------------------------------------------------

    /// Discover from `PYO3_CROSS_LIB_DIR` (build-details.json or sysconfigdata).
    fn discover_from_cross_lib_dir(&self, cross_lib_path: &Path) -> Result<DiscoveryResult> {
        if let Some(build_details_path) = find_build_details(cross_lib_path) {
            eprintln!("🐍 Using build-details.json for cross-compiling preparation");
            let config = parse_build_details_json_file(&build_details_path)?;
            let host_python = self.find_host_python()?;
            let soabi = soabi_from_ext_suffix(&config.ext_suffix);
            let implementation_name = config.interpreter_kind.to_string().to_ascii_lowercase();
            let interp = PythonInterpreter {
                config,
                executable: PathBuf::new(),
                platform: None,
                runnable: false,
                implementation_name,
                soabi,
            };
            Ok(DiscoveryResult {
                candidates: vec![Candidate::cross_compile_lib(interp)],
                host_python: Some(host_python),
            })
        } else {
            let host_python = self.find_host_python()?;
            eprintln!("🐍 Using host {host_python} for cross-compiling preparation");
            let sysconfig_path = find_sysconfigdata(cross_lib_path, self.target)?;
            let sysconfig_data = parse_sysconfigdata(&host_python, sysconfig_path)?;
            let interps = self.interpreter_from_sysconfigdata(&sysconfig_data)?;
            Ok(DiscoveryResult {
                candidates: interps
                    .into_iter()
                    .map(Candidate::cross_compile_lib)
                    .collect(),
                host_python: Some(host_python),
            })
        }
    }

    // -----------------------------------------------------------------------
    // Discovery: cross-compile without PYO3_CROSS_LIB_DIR (non-abi3)
    // -----------------------------------------------------------------------

    /// Discover interpreters for cross-compilation without `PYO3_CROSS_LIB_DIR`.
    ///
    /// Uses maturin's bundled sysconfig data to construct non-runnable interpreters
    /// matching the target platform.
    fn discover_cross_sysconfig(&self) -> Result<DiscoveryResult> {
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
        Ok(DiscoveryResult {
            candidates: interpreters.into_iter().map(Candidate::sysconfig).collect(),
            host_python: None,
        })
    }

    // -----------------------------------------------------------------------
    // Discovery: native (also used for abi3 cross without lib dir)
    // -----------------------------------------------------------------------

    /// Discover interpreters on the host machine.
    ///
    /// For abi3 builds, falls back to sysconfig if real interpreters aren't found.
    fn discover_native(&self, fixed_abi3: Option<(u8, u8)>) -> Result<DiscoveryResult> {
        match self.find_native_interpreters() {
            Ok(interps) => Ok(DiscoveryResult {
                candidates: to_candidates(interps),
                host_python: None,
            }),
            Err(err) if fixed_abi3.is_some() => {
                // Abi3: try sysconfig fallback before giving up
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
                Ok(DiscoveryResult {
                    candidates: sysconfig_interps
                        .into_iter()
                        .map(Candidate::sysconfig)
                        .collect(),
                    host_python: None,
                })
            }
            Err(err) => Err(err),
        }
    }

    /// Find native interpreters: auto-discover, user-specified, or default.
    fn find_native_interpreters(&self) -> Result<Vec<PythonInterpreter>> {
        if self.find_interpreter {
            PythonInterpreter::find_all(self.target, self.bridge, self.requires_python)
                .context("Finding python interpreters failed")
        } else if !self.user_interpreters.is_empty() {
            find_interpreter(
                self.bridge,
                self.user_interpreters,
                self.target,
                self.requires_python,
                self.generate_import_lib,
            )
        } else {
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

    // -----------------------------------------------------------------------
    // Filtering: abi3 policy
    // -----------------------------------------------------------------------

    /// Filter candidates for abi3 builds.
    ///
    /// When building abi3 wheels, we prefer interpreters that support the stable API.
    /// Non-abi3-capable interpreters (PyPy, free-threaded CPython) are only included
    /// if explicitly requested by the user via `-i`.
    ///
    /// This fixes:
    /// - #2772: free-threaded interpreter chosen over non-free-threaded for abi3
    /// - #2852: unexpected PyPy wheel generated for abi3 cross-compile
    /// - #2607: PyPy from `-i` is now honored (not silently dropped)
    fn filter_for_abi3(&self, candidates: Vec<Candidate>) -> Vec<Candidate> {
        if candidates.is_empty() {
            return candidates;
        }

        let user_requested_pypy = self.user_requested_pypy();
        let user_requested_free_threaded = self.user_requested_free_threaded();

        let (abi3_capable, non_abi3): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .partition(|c| c.interpreter.has_stable_api());

        let mut result = abi3_capable;

        // Only include non-abi3-capable interpreters if explicitly requested
        for candidate in non_abi3 {
            let excluded = match candidate.interpreter.interpreter_kind {
                InterpreterKind::PyPy => !user_requested_pypy,
                InterpreterKind::CPython if candidate.interpreter.gil_disabled => {
                    !user_requested_free_threaded
                }
                _ => false,
            };
            if !excluded {
                result.push(candidate);
            }
        }

        result
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

    // -----------------------------------------------------------------------
    // Finalization
    // -----------------------------------------------------------------------

    /// Finalize abi3 resolution: apply platform-specific fallbacks.
    fn finalize_abi3(
        &self,
        candidates: Vec<Candidate>,
        major: u8,
        minor: u8,
    ) -> Result<Vec<PythonInterpreter>> {
        // Handle abi3 cross-compilation (PyPy → sysconfig)
        let candidates = if self.target.cross_compiling() {
            self.handle_abi3_cross(candidates)?
        } else {
            candidates
        };

        // Windows-specific abi3 handling
        if self.target.is_windows() {
            return self.finalize_abi3_windows(candidates, major, minor);
        }

        let interpreters = Self::candidates_to_interpreters(candidates);

        if !interpreters.is_empty() {
            if !self.target.cross_compiling() {
                self.print_found(&interpreters);
            }
            Ok(interpreters)
        } else if self.user_interpreters.is_empty() {
            eprintln!("🐍 Not using a specific python interpreter");
            Ok(vec![
                self.make_fake_interpreter(major as usize, minor as usize),
            ])
        } else {
            bail!("Failed to find any python interpreter");
        }
    }

    /// Handle abi3 cross-compilation: resolve PyPy through sysconfig.
    fn handle_abi3_cross(&self, candidates: Vec<Candidate>) -> Result<Vec<Candidate>> {
        let mut interps = Vec::with_capacity(candidates.len());
        let mut pypys = Vec::new();
        for candidate in candidates {
            if candidate.interpreter.interpreter_kind.is_pypy() {
                // Only include PyPy in cross-compile abi3 if explicitly requested (#2852)
                if self.user_requested_pypy() {
                    pypys.push(PathBuf::from(format!(
                        "pypy{}.{}",
                        candidate.interpreter.major, candidate.interpreter.minor
                    )));
                }
            } else {
                interps.push(candidate);
            }
        }
        // Cross-compiling to PyPy with abi3: can't use host pypy, use sysconfig
        if !pypys.is_empty() {
            let sysconfig_interps = find_interpreter_in_sysconfig(
                self.bridge,
                &pypys,
                self.target,
                self.requires_python,
            )?;
            interps.extend(sysconfig_interps.into_iter().map(Candidate::sysconfig));
        }
        if interps.is_empty() {
            bail!("Failed to find any python interpreter");
        }
        Ok(interps)
    }

    /// Finalize abi3 on Windows.
    ///
    /// Note: `PYO3_CROSS_LIB_DIR` and `PYO3_CONFIG_FILE` are already handled
    /// earlier in the unified pipeline.
    fn finalize_abi3_windows(
        &self,
        candidates: Vec<Candidate>,
        major: u8,
        minor: u8,
    ) -> Result<Vec<PythonInterpreter>> {
        let interpreters = Self::candidates_to_interpreters(candidates);

        if self.generate_import_lib {
            eprintln!(
                "🐍 Not using a specific python interpreter \
                 (automatically generating windows import library)"
            );
            let mut result = interpreters;
            if result.is_empty() {
                result.push(self.make_fake_interpreter(major as usize, minor as usize));
            }
            return Ok(result);
        }

        if interpreters.is_empty() {
            bail!("Failed to find any python interpreter");
        }
        Ok(interpreters)
    }

    // -----------------------------------------------------------------------
    // Shared helpers
    // -----------------------------------------------------------------------

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

    /// Convert candidates to interpreters, discarding source info.
    fn candidates_to_interpreters(candidates: Vec<Candidate>) -> Vec<PythonInterpreter> {
        candidates
            .into_iter()
            .map(Candidate::into_interpreter)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Helper functions (kept as module-level for reuse)
// ---------------------------------------------------------------------------

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
