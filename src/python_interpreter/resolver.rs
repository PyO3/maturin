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

/// How a candidate Python interpreter was discovered.
///
/// Tracks provenance so the pipeline can make informed decisions
/// (e.g. whether to set `PYO3_PYTHON`, whether the interpreter is runnable).
#[derive(Debug, Clone, PartialEq, Eq)]
enum CandidateSource {
    /// A real executable on the host machine.
    Executable,
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
struct Candidate {
    interpreter: PythonInterpreter,
    source: CandidateSource,
}

impl Candidate {
    /// Classify existing `PythonInterpreter`s as candidates based on `runnable`.
    fn from_interpreters(interps: Vec<PythonInterpreter>) -> Vec<Self> {
        interps
            .into_iter()
            .map(|interpreter| {
                let source = if interpreter.runnable {
                    CandidateSource::Executable
                } else {
                    CandidateSource::Sysconfig
                };
                Candidate {
                    interpreter,
                    source,
                }
            })
            .collect()
    }

    /// Convert this candidate into a `PythonInterpreter`, discarding source info.
    fn into_interpreter(self) -> PythonInterpreter {
        self.interpreter
    }
}

/// Discovery result: `(candidates, host_python)`.
///
/// `host_python` is only set during cross-compilation (from `PYO3_CROSS_LIB_DIR`)
/// and is used by the caller to set `PYO3_PYTHON` / `PYTHON_SYS_EXECUTABLE`.
type DiscoveryResult = (Vec<Candidate>, Option<PythonInterpreter>);

/// A parsed interpreter specification from a user-provided string.
///
/// Handles formats like `"python3.14t"`, `"pypy3.11"`, `"graalpy-3.10"`, `"3.9"`.
#[derive(Debug, Clone)]
struct InterpreterSpec {
    kind: InterpreterKind,
    major: usize,
    minor: usize,
    abiflags: String,
}

impl InterpreterSpec {
    /// Parse a user-provided interpreter string.
    ///
    /// Accepts:
    /// - `pypy3.11` or `pypy-3.11`
    /// - `graalpy3.10` or `graalpy-3.10`
    /// - `python3.14t` or `python-3.14t`
    /// - `3.9` or `3.14t` (bare version, assumes CPython)
    ///
    /// Returns `None` for version-less names like `"pypy"` or `"python"`.
    /// Returns `Ok(None)` for file paths or unrecognized formats.
    fn parse(s: &str) -> Result<Option<Self>> {
        let (kind, ver_str) = if let Some(ver) = s.strip_prefix("pypy") {
            (InterpreterKind::PyPy, ver.strip_prefix('-').unwrap_or(ver))
        } else if let Some(ver) = s.strip_prefix("graalpy") {
            (
                InterpreterKind::GraalPy,
                ver.strip_prefix('-').unwrap_or(ver),
            )
        } else if let Some(ver) = s.strip_prefix("python") {
            (
                InterpreterKind::CPython,
                ver.strip_prefix('-').unwrap_or(ver),
            )
        } else if s.starts_with(|c: char| c.is_ascii_digit()) {
            (InterpreterKind::CPython, s)
        } else {
            // File paths or unrecognized names (e.g. "jython3.9") are not
            // interpreter specs — return None so callers can handle them.
            return Ok(None);
        };

        if ver_str.is_empty() || !ver_str.starts_with(|c: char| c.is_ascii_digit()) {
            return Ok(None);
        }

        let (ver_str, abiflags) = if let Some(v) = ver_str.strip_suffix('t') {
            (v, "t")
        } else {
            (ver_str, "")
        };

        // PyPy / GraalPy don't support free-threaded builds
        if !matches!(kind, InterpreterKind::CPython) && abiflags == "t" {
            bail!("Free-threaded builds are only supported for CPython, not {kind}");
        }

        let (major_str, minor_str) = match ver_str.split_once('.') {
            Some(parts) => parts,
            None => return Ok(None), // e.g. "python3" — no minor version
        };
        let major = major_str.parse::<usize>().with_context(|| {
            format!("Invalid python interpreter major version '{major_str}', expect a digit")
        })?;
        let minor = minor_str.parse::<usize>().with_context(|| {
            format!("Invalid python interpreter minor version '{minor_str}', expect a digit")
        })?;

        if (major, minor) < (3, 13) && abiflags == "t" {
            bail!("Free-threaded Python interpreter is only supported on 3.13 and later.");
        }

        Ok(Some(InterpreterSpec {
            kind,
            major,
            minor,
            abiflags: abiflags.to_string(),
        }))
    }

    /// Try to parse a filename (not a full path) for kind/abiflags detection.
    /// Returns `None` for unparsable names rather than erroring.
    fn try_parse_filename(s: &str) -> Option<Self> {
        // Ignore errors (file paths, unsupported formats) — just return None
        Self::parse(s).ok().flatten()
    }
}

/// Resolves which Python interpreters to build wheels for.
///
/// Given a bridge model, target platform, and optional user-specified
/// interpreters, discovers and validates the set of Python interpreters
/// that should be used for the build. Handles all combinations of:
///
/// - abi3 vs non-abi3
/// - cross-compile vs native
/// - Windows vs Unix
/// - user-specified interpreters vs auto-discovery
///
/// Entry point: [`resolve()`](Self::resolve), which returns the list of
/// interpreters and an optional host python path for cross-compilation.
pub struct InterpreterResolver<'a> {
    target: &'a Target,
    bridge: &'a BridgeModel,
    requires_python: Option<&'a VersionSpecifiers>,
    user_interpreters: &'a [PathBuf],
    find_interpreter: bool,
    generate_import_lib: bool,
}

impl<'a> InterpreterResolver<'a> {
    /// Create a new interpreter resolver with the given build context.
    pub fn new(
        target: &'a Target,
        bridge: &'a BridgeModel,
        requires_python: Option<&'a VersionSpecifiers>,
        user_interpreters: &'a [PathBuf],
        find_interpreter: bool,
        generate_import_lib: bool,
    ) -> Self {
        Self {
            target,
            bridge,
            requires_python,
            user_interpreters,
            find_interpreter,
            generate_import_lib,
        }
    }

    /// Main entry point: resolve the list of Python interpreters to build for.
    ///
    /// Returns the resolved interpreters and, for cross-compilation, the host
    /// Python path that should be used to set `PYO3_PYTHON`.
    pub fn resolve(&self) -> Result<(Vec<PythonInterpreter>, Option<PathBuf>)> {
        match self.bridge {
            BridgeModel::Cffi => self.resolve_single("cffi").map(|i| (vec![i], None)),
            BridgeModel::Bin(None) | BridgeModel::UniFfi => Ok((vec![], None)),
            BridgeModel::PyO3(pyo3) | BridgeModel::Bin(Some(pyo3)) => self.resolve_pyo3(pyo3),
        }
    }

    /// Resolve interpreters for pyo3/pyo3-ffi bindings (including Bin(Some(pyo3))).
    ///
    /// Follows a unified pipeline regardless of native/cross, abi3/non-abi3:
    /// 1. Check `PYO3_CONFIG_FILE` (explicit override)
    /// 2. Discover candidates
    /// 3. Set `PYO3_PYTHON` if cross-compiling with a host interpreter
    /// 4. Filter for abi3 (if applicable)
    /// 5. Finalize: apply fallback policies, validate result
    fn resolve_pyo3(&self, pyo3: &PyO3) -> Result<(Vec<PythonInterpreter>, Option<PathBuf>)> {
        // Step 1: PYO3_CONFIG_FILE is an explicit override that trumps everything
        if let Some(config_file) = env::var_os("PYO3_CONFIG_FILE") {
            let config = InterpreterConfig::from_pyo3_config(config_file.as_ref(), self.target)
                .context("Invalid PYO3_CONFIG_FILE")?;
            return Ok((vec![PythonInterpreter::from_config(config)], None));
        }

        let fixed_abi3 = match &pyo3.abi3 {
            Some(Abi3Version::Version(major, minor)) => Some((*major, *minor)),
            _ => None,
        };

        // Step 2: Discover candidates (unified for native/cross)
        let (candidates, host_python_interp) = self.discover_candidates(fixed_abi3)?;

        // Capture host_python for the caller to set PYO3_PYTHON
        let host_python = host_python_interp.as_ref().map(|h| h.executable.clone());

        // Step 3-4: Filter and finalize (differs for abi3 vs non-abi3)
        let interpreters = if let Some((major, minor)) = fixed_abi3 {
            // Filter out non-abi3-capable interpreters (PyPy, free-threaded)
            // unless the user explicitly provided them via `-i`.
            // When `develop` passes a venv python, user_interpreters is non-empty
            // so we trust that choice (the filename is just "python" and doesn't
            // encode interpreter kind/flags).
            let filtered = if !self.find_interpreter && !self.user_interpreters.is_empty() {
                candidates
            } else {
                self.filter_for_abi3(candidates)
            };
            self.finalize_abi3(filtered, major, minor)?
        } else {
            Self::print_found_candidates(&candidates);
            Self::candidates_to_interpreters(candidates)
        };

        Ok((interpreters, host_python))
    }

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
            // Abi3 Windows cross with PYO3_CROSS_LIB_DIR: use a placeholder
            if let Some((major, minor)) = fixed_abi3
                && self.target.is_windows()
            {
                return Ok((
                    vec![Candidate {
                        interpreter: PythonInterpreter::placeholder(
                            major as usize,
                            minor as usize,
                            self.target,
                        ),
                        source: CandidateSource::Placeholder,
                    }],
                    None,
                ));
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

    /// Discover from `PYO3_CROSS_LIB_DIR` (build-details.json or sysconfigdata).
    fn discover_from_cross_lib_dir(&self, cross_lib_path: &Path) -> Result<DiscoveryResult> {
        if let Some(build_details_path) = find_build_details(cross_lib_path) {
            eprintln!("🐍 Using build-details.json for cross-compiling preparation");
            let config = parse_build_details_json_file(&build_details_path)?;
            let host_python = self.find_host_python()?;
            let soabi = InterpreterConfig::soabi_from_ext_suffix(&config.ext_suffix);
            let implementation_name = config.interpreter_kind.to_string().to_ascii_lowercase();
            let interp = PythonInterpreter {
                config,
                executable: PathBuf::new(),
                platform: None,
                runnable: false,
                implementation_name,
                soabi,
            };
            Ok((
                vec![Candidate {
                    interpreter: interp,
                    source: CandidateSource::CrossCompileLib,
                }],
                Some(host_python),
            ))
        } else {
            let host_python = self.find_host_python()?;
            eprintln!("🐍 Using host {host_python} for cross-compiling preparation");
            let sysconfig_path = find_sysconfigdata(cross_lib_path, self.target)?;
            let sysconfig_data = parse_sysconfigdata(&host_python, sysconfig_path)?;
            let interpreter = self.interpreter_from_sysconfigdata(&sysconfig_data)?;
            Ok((
                vec![Candidate {
                    interpreter,
                    source: CandidateSource::CrossCompileLib,
                }],
                Some(host_python),
            ))
        }
    }

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
                && super::discovery::check_executable(interp, self.target, self.bridge)?.is_none()
            {
                bail!("{} is not a valid python interpreter", interp.display());
            }
        }

        let interpreters = self.find_in_sysconfig(self.user_interpreters)?;
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
        Ok((
            interpreters
                .into_iter()
                .map(|interpreter| Candidate {
                    interpreter,
                    source: CandidateSource::Sysconfig,
                })
                .collect(),
            None,
        ))
    }

    /// Discover interpreters on the host machine.
    ///
    /// 1. Try to find real interpreters (auto-discover or check user-specified)
    /// 2. Fall back to bundled sysconfig for any that aren't found
    /// 3. For abi3 builds, also try a broader sysconfig fallback if nothing works
    fn discover_native(&self, fixed_abi3: Option<(u8, u8)>) -> Result<DiscoveryResult> {
        // --- Step 1+2: Find real interpreters with per-interpreter sysconfig fallback ---
        let found = if self.find_interpreter {
            super::discovery::find_all(self.target, self.bridge, self.requires_python)
                .context("Finding python interpreters failed")?
        } else {
            self.find_specified_interpreters()?
        };

        if !found.is_empty() {
            return Ok((Candidate::from_interpreters(found), None));
        }

        // --- Step 3: Nothing found — try abi3 sysconfig fallback ---
        if fixed_abi3.is_some() {
            if self.target.is_windows() && !self.generate_import_lib {
                bail!(
                    "Need a Python interpreter to compile for Windows without \
                     PyO3's `generate-import-lib` feature"
                );
            }
            let sysconfig_result = self.find_in_sysconfig(self.user_interpreters);
            // Auto-discovery: swallow errors, fall through to placeholder.
            // User-specified interpreters: propagate parse errors.
            let sysconfig_interps = if self.user_interpreters.is_empty() {
                sysconfig_result.unwrap_or_default()
            } else {
                sysconfig_result?
            };
            // If the user specified interpreters and sysconfig didn't find them
            // either, fall through to the error below rather than silently
            // returning an empty list.
            if !sysconfig_interps.is_empty() || self.user_interpreters.is_empty() {
                return Ok((
                    sysconfig_interps
                        .into_iter()
                        .map(|interpreter| Candidate {
                            interpreter,
                            source: CandidateSource::Sysconfig,
                        })
                        .collect(),
                    None,
                ));
            }
        }

        // --- Error: nothing found anywhere ---
        if self.find_interpreter {
            if let Some(requires_python) = self.requires_python {
                bail!(
                    "Couldn't find any python interpreters with {requires_python}. \
                     Please specify at least one with -i"
                );
            } else {
                bail!(
                    "Couldn't find any python interpreters. \
                     Please specify at least one with -i"
                );
            }
        } else {
            let default_python;
            let to_check: &[PathBuf] = if !self.user_interpreters.is_empty() {
                self.user_interpreters
            } else {
                default_python = self.get_default_python();
                std::slice::from_ref(&default_python)
            };
            let interps_str = to_check
                .iter()
                .map(|path| format!("'{}'", path.display()))
                .collect::<Vec<_>>()
                .join(", ");
            bail!("Couldn't find any python interpreters from {interps_str}.");
        }
    }

    /// Check user-specified (or default) interpreters, with per-interpreter
    /// sysconfig fallback for any that aren't found on disk.
    fn find_specified_interpreters(&self) -> Result<Vec<PythonInterpreter>> {
        let default_python;
        let to_check: &[PathBuf] = if !self.user_interpreters.is_empty() {
            self.user_interpreters
        } else {
            default_python = self.get_default_python();
            std::slice::from_ref(&default_python)
        };

        let mut found = Vec::new();
        let mut missing = Vec::new();
        for interp in to_check {
            match super::discovery::check_executable(interp.clone(), self.target, self.bridge)? {
                Some(interp) => found.push(interp),
                None => missing.push(interp.clone()),
            }
        }

        if !missing.is_empty() {
            let sysconfig_interps = self.find_in_sysconfig(&missing)?;
            if !sysconfig_interps.is_empty()
                && self.target.is_windows()
                && !self.generate_import_lib
            {
                let names = sysconfig_interps
                    .iter()
                    .map(|i| format!("{} {}.{}", i.interpreter_kind, i.major, i.minor))
                    .collect::<Vec<_>>();
                bail!(
                    "Interpreters {names:?} were found in maturin's bundled sysconfig, \
                     but compiling for Windows without an interpreter requires \
                     PyO3's `generate-import-lib` feature"
                );
            }
            found.extend(sysconfig_interps);
        }

        Ok(found)
    }

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
        self.user_interpreters.iter().any(|p| {
            let name = p
                .file_name()
                .map(|f| f.to_string_lossy())
                .unwrap_or_default();
            // Check parsed spec first, then fall back to prefix match for
            // version-less names like "pypy" or "/usr/bin/pypy"
            InterpreterSpec::try_parse_filename(&name)
                .is_some_and(|s| s.kind == InterpreterKind::PyPy)
                || name.starts_with("pypy")
        })
    }

    /// Check if any user-specified interpreter looks like free-threaded Python.
    fn user_requested_free_threaded(&self) -> bool {
        self.user_interpreters.iter().any(|p| {
            let name = p
                .file_name()
                .map(|f| f.to_string_lossy())
                .unwrap_or_default();
            InterpreterSpec::try_parse_filename(&name).is_some_and(|s| s.abiflags == "t")
        })
    }

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

        if !candidates.is_empty() {
            Self::print_found_candidates(&candidates);
            Ok(Self::candidates_to_interpreters(candidates))
        } else if self.user_interpreters.is_empty() {
            eprintln!("🐍 Not using a specific python interpreter");
            Ok(vec![PythonInterpreter::placeholder(
                major as usize,
                minor as usize,
                self.target,
            )])
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
            let sysconfig_interps = self.find_in_sysconfig(&pypys)?;
            interps.extend(sysconfig_interps.into_iter().map(|interpreter| Candidate {
                interpreter,
                source: CandidateSource::Sysconfig,
            }));
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
                result.push(PythonInterpreter::placeholder(
                    major as usize,
                    minor as usize,
                    self.target,
                ));
            }
            return Ok(result);
        }

        if interpreters.is_empty() {
            bail!("Failed to find any python interpreter");
        }
        Ok(interpreters)
    }

    /// Find a host Python interpreter for cross-compilation.
    fn find_host_python(&self) -> Result<PythonInterpreter> {
        let interpreters = if !self.user_interpreters.is_empty() {
            super::discovery::check_executables(self.user_interpreters, self.target, self.bridge)?
        } else {
            super::discovery::find_all(self.target, self.bridge, self.requires_python)
                .context("Finding python interpreters failed")?
        };

        if interpreters.is_empty() {
            if let Some(requires_python) = self.requires_python {
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
        Ok(interpreters
            .into_iter()
            .next()
            .expect("checked non-empty above"))
    }

    /// Find python interpreters in the bundled sysconfig.
    ///
    /// When `interpreters` is empty, returns all interpreters matching the target.
    /// Otherwise, parses each entry as an [`InterpreterSpec`] and looks up the
    /// corresponding sysconfig.
    fn find_in_sysconfig(&self, interpreters: &[PathBuf]) -> Result<Vec<PythonInterpreter>> {
        if interpreters.is_empty() {
            return Ok(super::discovery::lookup_target(
                self.target,
                self.requires_python,
                Some(self.bridge),
            ));
        }
        let mut result = Vec::new();
        for interp in interpreters {
            let interp_display = interp.display().to_string();
            let python_name = interp
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&interp_display);
            let spec = match InterpreterSpec::parse(python_name)? {
                Some(spec) => spec,
                None => {
                    // version-less name like bare "pypy" — warn the user
                    eprintln!(
                        "⚠️  Warning: Skipping '{interp_display}': could not determine version \
                         from interpreter name '{python_name}'. \
                         Specify a version like '{python_name}3.11'."
                    );
                    continue;
                }
            };
            let sysconfig = InterpreterConfig::lookup_one(
                self.target,
                spec.kind,
                (spec.major, spec.minor),
                &spec.abiflags,
            )
            .with_context(|| {
                format!(
                    "Failed to find a {} {}.{} interpreter in known sysconfig",
                    spec.kind, spec.major, spec.minor
                )
            })?;
            debug!(
                "Found {} {}.{}{} in bundled sysconfig",
                sysconfig.interpreter_kind, sysconfig.major, sysconfig.minor, sysconfig.abiflags
            );
            result.push(PythonInterpreter::from_config(sysconfig.clone()));
        }
        Ok(result)
    }

    /// Build a PythonInterpreter from sysconfigdata.
    fn interpreter_from_sysconfigdata(
        &self,
        data: &std::collections::HashMap<String, String>,
    ) -> Result<PythonInterpreter> {
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
        Ok(PythonInterpreter {
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
        })
    }

    /// Resolve a single interpreter for cffi or similar.
    fn resolve_single(&self, bridge_name: &str) -> Result<PythonInterpreter> {
        let executable = if self.user_interpreters.is_empty() {
            self.target.get_python()
        } else if self.user_interpreters.len() == 1 {
            self.user_interpreters[0].clone()
        } else {
            bail!(
                "You can only specify one python interpreter for {}",
                bridge_name
            );
        };
        let err_message = format!(
            "Failed to find a python interpreter from `{}`",
            executable.display()
        );
        let interp = super::discovery::check_executable(executable, self.target, self.bridge)
            .context(format_err!(err_message.clone()))?
            .ok_or_else(|| format_err!(err_message))?;
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

    /// Print "Found ..." for candidates discovered from executables or sysconfig.
    ///
    /// Candidates from `CrossCompileLib` and `Placeholder` sources are skipped
    /// since they have their own messages printed during discovery/finalization.
    fn print_found_candidates(candidates: &[Candidate]) {
        let displayable: Vec<_> = candidates
            .iter()
            .filter(|c| {
                matches!(
                    c.source,
                    CandidateSource::Executable | CandidateSource::Sysconfig
                )
            })
            .map(|c| c.interpreter.to_string())
            .collect();
        if !displayable.is_empty() {
            eprintln!("🐍 Found {}", displayable.join(", "));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interpreter_spec_cpython_versions() {
        let spec = InterpreterSpec::parse("python3.9").unwrap().unwrap();
        assert_eq!(spec.kind, InterpreterKind::CPython);
        assert_eq!(spec.major, 3);
        assert_eq!(spec.minor, 9);
        assert_eq!(spec.abiflags, "");

        let spec = InterpreterSpec::parse("python-3.12").unwrap().unwrap();
        assert_eq!(spec.kind, InterpreterKind::CPython);
        assert_eq!(spec.major, 3);
        assert_eq!(spec.minor, 12);
        assert_eq!(spec.abiflags, "");
    }

    #[test]
    fn test_interpreter_spec_bare_version() {
        let spec = InterpreterSpec::parse("3.9").unwrap().unwrap();
        assert_eq!(spec.kind, InterpreterKind::CPython);
        assert_eq!(spec.major, 3);
        assert_eq!(spec.minor, 9);
        assert_eq!(spec.abiflags, "");

        let spec = InterpreterSpec::parse("3.14").unwrap().unwrap();
        assert_eq!(spec.major, 3);
        assert_eq!(spec.minor, 14);
    }

    #[test]
    fn test_interpreter_spec_free_threaded() {
        let spec = InterpreterSpec::parse("python3.14t").unwrap().unwrap();
        assert_eq!(spec.kind, InterpreterKind::CPython);
        assert_eq!(spec.major, 3);
        assert_eq!(spec.minor, 14);
        assert_eq!(spec.abiflags, "t");

        let spec = InterpreterSpec::parse("3.13t").unwrap().unwrap();
        assert_eq!(spec.kind, InterpreterKind::CPython);
        assert_eq!(spec.major, 3);
        assert_eq!(spec.minor, 13);
        assert_eq!(spec.abiflags, "t");
    }

    #[test]
    fn test_interpreter_spec_free_threaded_too_old() {
        let err = InterpreterSpec::parse("python3.12t").unwrap_err();
        assert!(
            err.to_string().contains("3.13"),
            "expected version constraint in error: {err}"
        );
    }

    #[test]
    fn test_interpreter_spec_pypy() {
        let spec = InterpreterSpec::parse("pypy3.11").unwrap().unwrap();
        assert_eq!(spec.kind, InterpreterKind::PyPy);
        assert_eq!(spec.major, 3);
        assert_eq!(spec.minor, 11);
        assert_eq!(spec.abiflags, "");

        let spec = InterpreterSpec::parse("pypy-3.10").unwrap().unwrap();
        assert_eq!(spec.kind, InterpreterKind::PyPy);
        assert_eq!(spec.major, 3);
        assert_eq!(spec.minor, 10);
    }

    #[test]
    fn test_interpreter_spec_pypy_free_threaded_rejected() {
        let err = InterpreterSpec::parse("pypy3.11t").unwrap_err();
        assert!(
            err.to_string().contains("Free-threaded"),
            "expected free-threaded error: {err}"
        );
    }

    #[test]
    fn test_interpreter_spec_graalpy() {
        let spec = InterpreterSpec::parse("graalpy3.10").unwrap().unwrap();
        assert_eq!(spec.kind, InterpreterKind::GraalPy);
        assert_eq!(spec.major, 3);
        assert_eq!(spec.minor, 10);

        let spec = InterpreterSpec::parse("graalpy-3.10").unwrap().unwrap();
        assert_eq!(spec.kind, InterpreterKind::GraalPy);
        assert_eq!(spec.major, 3);
        assert_eq!(spec.minor, 10);
    }

    #[test]
    fn test_interpreter_spec_graalpy_free_threaded_rejected() {
        let err = InterpreterSpec::parse("graalpy3.10t").unwrap_err();
        assert!(
            err.to_string().contains("Free-threaded"),
            "expected free-threaded error: {err}"
        );
    }

    #[test]
    fn test_interpreter_spec_version_less_returns_none() {
        assert!(InterpreterSpec::parse("python").unwrap().is_none());
        assert!(InterpreterSpec::parse("pypy").unwrap().is_none());
        assert!(InterpreterSpec::parse("graalpy").unwrap().is_none());
    }

    #[test]
    fn test_interpreter_spec_unrecognized_name() {
        // Unrecognized interpreter names return None, not an error
        assert!(InterpreterSpec::parse("jython3.9").unwrap().is_none());
        assert!(
            InterpreterSpec::parse("/usr/bin/python3")
                .unwrap()
                .is_none()
        );
        // Windows executable names and bare "python3" without minor version
        assert!(InterpreterSpec::parse("python.exe").unwrap().is_none());
        assert!(InterpreterSpec::parse("python3").unwrap().is_none());
    }

    #[test]
    fn test_interpreter_spec_try_parse_filename() {
        let spec = InterpreterSpec::try_parse_filename("python3.14t").unwrap();
        assert_eq!(spec.kind, InterpreterKind::CPython);
        assert_eq!(spec.abiflags, "t");

        let spec = InterpreterSpec::try_parse_filename("pypy3.10").unwrap();
        assert_eq!(spec.kind, InterpreterKind::PyPy);

        // version-less names return None
        assert!(InterpreterSpec::try_parse_filename("python").is_none());

        // unsupported names return None (not error)
        assert!(InterpreterSpec::try_parse_filename("jython3.9").is_none());
    }
}
