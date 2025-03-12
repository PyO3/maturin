use crate::auditwheel::{AuditWheelMode, PlatformTag};
use crate::compile::{CompileTarget, LIB_CRATE_TYPES};
use crate::cross_compile::{find_sysconfigdata, parse_sysconfigdata};
use crate::project_layout::ProjectResolver;
use crate::pyproject_toml::ToolMaturin;
use crate::python_interpreter::{InterpreterConfig, InterpreterKind};
use crate::{Bindings, BridgeModel, BuildContext, PythonInterpreter, Target};
use anyhow::{bail, format_err, Context, Result};
use cargo_metadata::{CrateType, PackageId, TargetKind};
use cargo_metadata::{Metadata, Node};
use cargo_options::heading;
use pep440_rs::VersionSpecifiers;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use tracing::{debug, instrument};

// This is used for BridgeModel::Bindings("pyo3-ffi") and BridgeModel::Bindings("pyo3").
// These should be treated almost identically but must be correctly identified
// as one or the other in logs. pyo3-ffi is ordered first because it is newer
// and more restrictive.
const PYO3_BINDING_CRATES: [&str; 2] = ["pyo3-ffi", "pyo3"];

/// Cargo options for the build process
#[derive(Debug, Default, Serialize, Deserialize, clap::Parser, Clone, Eq, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
pub struct CargoOptions {
    /// Do not print cargo log messages
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Number of parallel jobs, defaults to # of CPUs
    #[arg(short = 'j', long, value_name = "N", help_heading = heading::COMPILATION_OPTIONS)]
    pub jobs: Option<usize>,

    /// Build artifacts with the specified Cargo profile
    #[arg(long, value_name = "PROFILE-NAME", help_heading = heading::COMPILATION_OPTIONS)]
    pub profile: Option<String>,

    /// Space or comma separated list of features to activate
    #[arg(
        short = 'F',
        long,
        action = clap::ArgAction::Append,
        help_heading = heading::FEATURE_SELECTION,
    )]
    pub features: Vec<String>,

    /// Activate all available features
    #[arg(long, help_heading = heading::FEATURE_SELECTION)]
    pub all_features: bool,

    /// Do not activate the `default` feature
    #[arg(long, help_heading = heading::FEATURE_SELECTION)]
    pub no_default_features: bool,

    /// Build for the target triple
    #[arg(
        long,
        value_name = "TRIPLE",
        env = "CARGO_BUILD_TARGET",
        help_heading = heading::COMPILATION_OPTIONS,
    )]
    pub target: Option<String>,

    /// Directory for all generated artifacts
    #[arg(long, value_name = "DIRECTORY", help_heading = heading::COMPILATION_OPTIONS)]
    pub target_dir: Option<PathBuf>,

    /// Path to Cargo.toml
    #[arg(short = 'm', long, value_name = "PATH", help_heading = heading::MANIFEST_OPTIONS)]
    pub manifest_path: Option<PathBuf>,

    /// Ignore `rust-version` specification in packages
    #[arg(long)]
    pub ignore_rust_version: bool,

    /// Use verbose output (-vv very verbose/build.rs output)
    // Note that this duplicates the global option, but clap seems to be fine with that.
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Coloring: auto, always, never
    #[arg(long, value_name = "WHEN")]
    pub color: Option<String>,

    /// Require Cargo.lock and cache are up to date
    #[arg(long, help_heading = heading::MANIFEST_OPTIONS)]
    pub frozen: bool,

    /// Require Cargo.lock is up to date
    #[arg(long, help_heading = heading::MANIFEST_OPTIONS)]
    pub locked: bool,

    /// Run without accessing the network
    #[arg(long, help_heading = heading::MANIFEST_OPTIONS)]
    pub offline: bool,

    /// Override a configuration value (unstable)
    #[arg(long, value_name = "KEY=VALUE", action = clap::ArgAction::Append)]
    pub config: Vec<String>,

    /// Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details
    #[arg(short = 'Z', value_name = "FLAG", action = clap::ArgAction::Append)]
    pub unstable_flags: Vec<String>,

    /// Timing output formats (unstable) (comma separated): html, json
    #[arg(
        long,
        value_name = "FMTS",
        value_delimiter = ',',
        require_equals = true,
        help_heading = heading::COMPILATION_OPTIONS,
    )]
    pub timings: Option<Vec<String>>,

    /// Outputs a future incompatibility report at the end of the build (unstable)
    #[arg(long)]
    pub future_incompat_report: bool,

    /// Rustc flags
    #[arg(num_args = 0.., trailing_var_arg = true)]
    pub args: Vec<String>,
}

/// High level API for building wheels from a crate which is also used for the CLI
#[derive(Debug, Default, Serialize, Deserialize, clap::Parser, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct BuildOptions {
    /// Control the platform tag on linux.
    ///
    /// Options are `manylinux` tags (for example `manylinux2014`/`manylinux_2_24`)
    /// or `musllinux` tags (for example `musllinux_1_2`)
    /// and `linux` for the native linux tag.
    ///
    /// Note that `manylinux1` and `manylinux2010` is unsupported by the rust compiler.
    /// Wheels with the native `linux` tag will be rejected by pypi,
    /// unless they are separately validated by `auditwheel`.
    ///
    /// The default is the lowest compatible `manylinux` tag, or plain `linux` if nothing matched
    ///
    /// This option is ignored on all non-linux platforms
    #[arg(
        id = "compatibility",
        long = "compatibility",
        alias = "manylinux",
        num_args = 0..,
        action = clap::ArgAction::Append
    )]
    pub platform_tag: Vec<PlatformTag>,

    /// The python versions to build wheels for, given as the executables of
    /// interpreters such as `python3.9` or `/usr/bin/python3.8`.
    #[arg(short, long, num_args = 0.., action = clap::ArgAction::Append)]
    pub interpreter: Vec<PathBuf>,

    /// Find interpreters from the host machine
    #[arg(short = 'f', long, conflicts_with = "interpreter")]
    pub find_interpreter: bool,

    /// Which kind of bindings to use.
    #[arg(short, long, value_parser = ["pyo3", "pyo3-ffi", "cffi", "uniffi", "bin"])]
    pub bindings: Option<String>,

    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    #[arg(short, long)]
    pub out: Option<PathBuf>,

    /// Audit wheel for manylinux compliance
    #[arg(long, conflicts_with = "skip_auditwheel")]
    pub auditwheel: Option<AuditWheelMode>,

    /// Don't check for manylinux compliance
    #[arg(long, hide = true)]
    pub skip_auditwheel: bool,

    /// For manylinux targets, use zig to ensure compliance for the chosen manylinux version
    ///
    /// Default to manylinux2014/manylinux_2_17 if you do not specify an `--compatibility`
    ///
    /// Make sure you installed zig with `pip install maturin[zig]`
    #[cfg(feature = "zig")]
    #[arg(long)]
    pub zig: bool,

    /// Cargo build options
    #[command(flatten)]
    pub cargo: CargoOptions,
}

impl Deref for BuildOptions {
    type Target = CargoOptions;

    fn deref(&self) -> &Self::Target {
        &self.cargo
    }
}

impl DerefMut for BuildOptions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cargo
    }
}

impl BuildOptions {
    /// Finds the appropriate amount for python versions for each [BridgeModel].
    fn find_interpreters(
        &self,
        bridge: &BridgeModel,
        interpreter: &[PathBuf],
        target: &Target,
        requires_python: Option<&VersionSpecifiers>,
        generate_import_lib: bool,
    ) -> Result<Vec<PythonInterpreter>> {
        match bridge {
            BridgeModel::Bindings(Bindings {
                name: binding_name, ..
            })
            | BridgeModel::Bin(Some(Bindings {
                name: binding_name, ..
            })) => {
                let mut interpreters = Vec::new();
                if let Some(config_file) = env::var_os("PYO3_CONFIG_FILE") {
                    if !binding_name.starts_with("pyo3") {
                        bail!("Only pyo3 bindings can be configured with PYO3_CONFIG_FILE");
                    }
                    let interpreter_config =
                        InterpreterConfig::from_pyo3_config(config_file.as_ref(), target)
                            .context("Invalid PYO3_CONFIG_FILE")?;
                    interpreters.push(PythonInterpreter::from_config(interpreter_config));
                } else if binding_name.starts_with("pyo3") && target.cross_compiling() {
                    if let Some(cross_lib_dir) = env::var_os("PYO3_CROSS_LIB_DIR") {
                        let host_interpreters =
                            find_interpreter_in_host(bridge, interpreter, target, requires_python)?;
                        let host_python = &host_interpreters[0];
                        eprintln!("🐍 Using host {host_python} for cross-compiling preparation");
                        // pyo3
                        env::set_var("PYO3_PYTHON", &host_python.executable);
                        // rust-cpython, and legacy pyo3 versions
                        env::set_var("PYTHON_SYS_EXECUTABLE", &host_python.executable);

                        let sysconfig_path = find_sysconfigdata(cross_lib_dir.as_ref(), target)?;
                        env::set_var(
                            "MATURIN_PYTHON_SYSCONFIGDATA_DIR",
                            sysconfig_path.parent().unwrap(),
                        );

                        let sysconfig_data = parse_sysconfigdata(host_python, sysconfig_path)?;
                        let major = sysconfig_data
                            .get("version_major")
                            .context("version_major is not defined")?
                            .parse::<usize>()
                            .context("Could not parse value of version_major")?;
                        let minor = sysconfig_data
                            .get("version_minor")
                            .context("version_minor is not defined")?
                            .parse::<usize>()
                            .context("Could not parse value of version_minor")?;
                        let abiflags = sysconfig_data
                            .get("ABIFLAGS")
                            .map(ToString::to_string)
                            .unwrap_or_default();
                        let gil_disabled = sysconfig_data
                            .get("Py_GIL_DISABLED")
                            .map(|x| x == "1")
                            .unwrap_or_default();
                        let ext_suffix = sysconfig_data
                            .get("EXT_SUFFIX")
                            .context("syconfig didn't define an `EXT_SUFFIX` ಠ_ಠ")?;
                        let soabi = sysconfig_data.get("SOABI");
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
                        interpreters.push(PythonInterpreter {
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
                        });
                    } else {
                        if interpreter.is_empty() && !self.find_interpreter {
                            bail!("Couldn't find any python interpreters. Please specify at least one with -i");
                        }
                        for interp in interpreter {
                            // If `-i` looks like a file path, check if it's a valid interpreter
                            if interp.components().count() > 1
                                && PythonInterpreter::check_executable(interp, target, bridge)?
                                    .is_none()
                            {
                                bail!("{} is not a valid python interpreter", interp.display());
                            }
                        }
                        interpreters = find_interpreter_in_sysconfig(
                            bridge,
                            interpreter,
                            target,
                            requires_python,
                        )?;
                        if interpreters.is_empty() {
                            bail!(
                                "Couldn't find any python interpreters from '{}'. Please check that both major and minor python version have been specified in -i/--interpreter.",
                                interpreter
                                    .iter()
                                    .map(|p| p.display().to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }
                    }
                } else if binding_name.starts_with("pyo3") {
                    // Only pyo3/pyo3-ffi bindings supports bundled sysconfig interpreters
                    interpreters = find_interpreter(bridge, interpreter, target, requires_python)?;
                } else {
                    interpreters =
                        find_interpreter_in_host(bridge, interpreter, target, requires_python)?;
                }

                let interpreters_str = interpreters
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<String>>()
                    .join(", ");
                eprintln!("🐍 Found {interpreters_str}");

                Ok(interpreters)
            }
            BridgeModel::Cffi => {
                let interpreter =
                    find_single_python_interpreter(bridge, interpreter, target, "cffi")?;
                eprintln!("🐍 Using {interpreter} to generate the cffi bindings");
                Ok(vec![interpreter])
            }
            BridgeModel::Bin(None) | BridgeModel::UniFfi => Ok(vec![]),
            BridgeModel::BindingsAbi3 { major, minor, .. } => {
                let found_interpreters =
                    find_interpreter_in_host(bridge, interpreter, target, requires_python)
                        .or_else(|err| {
                            // Can only use sysconfig-derived interpreter on windows if generating the import lib
                            if target.is_windows() && !generate_import_lib {
                                return Err(err.context("Need a Python interpreter to compile for Windows without PyO3's `generate-import-lib` feature"));
                            }

                            let interps =
                                find_interpreter_in_sysconfig(bridge,interpreter, target, requires_python)
                                    .unwrap_or_default();
                            if interps.is_empty() && !self.interpreter.is_empty() {
                                // Print error when user supplied `--interpreter` option
                                Err(err)
                            } else {
                                Ok(interps)
                            }
                        })?;

                if target.is_windows() {
                    // On windows we might need a Python executable to locate a base prefix for
                    // linker args, if we're not using PyO3's `generate-import-lib` feature.
                    if env::var_os("PYO3_CROSS_LIB_DIR").is_some() {
                        // PYO3_CROSS_LIB_DIR should point to the `libs` directory inside base_prefix
                        // when cross compiling, so we fake a python interpreter matching it
                        eprintln!("⚠️  Cross-compiling is poorly supported");
                        Ok(vec![PythonInterpreter {
                            config: InterpreterConfig {
                                major: *major as usize,
                                minor: *minor as usize,
                                interpreter_kind: InterpreterKind::CPython,
                                abiflags: "".to_string(),
                                ext_suffix: ".pyd".to_string(),
                                pointer_width: None,
                                gil_disabled: false,
                            },
                            executable: PathBuf::new(),
                            platform: None,
                            runnable: false,
                            implementation_name: "cpython".to_string(),
                            soabi: None,
                        }])
                    } else if let Some(config_file) = env::var_os("PYO3_CONFIG_FILE") {
                        let interpreter_config =
                            InterpreterConfig::from_pyo3_config(config_file.as_ref(), target)
                                .context("Invalid PYO3_CONFIG_FILE")?;
                        Ok(vec![PythonInterpreter::from_config(interpreter_config)])
                    } else if generate_import_lib {
                        eprintln!("🐍 Not using a specific python interpreter (automatically generating windows import library)");
                        let mut found_interpreters = found_interpreters;
                        // fake a python interpreter if none directly found
                        if found_interpreters.is_empty() {
                            found_interpreters.push(PythonInterpreter {
                                config: InterpreterConfig {
                                    major: *major as usize,
                                    minor: *minor as usize,
                                    interpreter_kind: InterpreterKind::CPython,
                                    abiflags: "".to_string(),
                                    ext_suffix: ".pyd".to_string(),
                                    pointer_width: None,
                                    gil_disabled: false,
                                },
                                executable: PathBuf::new(),
                                platform: None,
                                runnable: false,
                                implementation_name: "cpython".to_string(),
                                soabi: None,
                            })
                        }
                        Ok(found_interpreters)
                    } else {
                        if found_interpreters.is_empty() {
                            bail!("Failed to find any python interpreter");
                        }
                        Ok(found_interpreters)
                    }
                } else {
                    eprintln!("🐍 Not using a specific python interpreter");
                    if self.interpreter.is_empty() {
                        // Fake one to make `BuildContext::build_wheels` happy for abi3 when no cpython/pypy found on host
                        // The python interpreter config doesn't matter, as it's not used for anything
                        Ok(vec![PythonInterpreter {
                            config: InterpreterConfig {
                                major: *major as usize,
                                minor: *minor as usize,
                                interpreter_kind: InterpreterKind::CPython,
                                abiflags: "".to_string(),
                                ext_suffix: "".to_string(),
                                pointer_width: None,
                                gil_disabled: false,
                            },
                            executable: PathBuf::new(),
                            platform: None,
                            runnable: false,
                            implementation_name: "cpython".to_string(),
                            soabi: None,
                        }])
                    } else if target.cross_compiling() {
                        let mut interps = Vec::with_capacity(found_interpreters.len());
                        let mut pypys = Vec::new();
                        for interp in found_interpreters {
                            if interp.interpreter_kind.is_pypy() {
                                pypys.push(PathBuf::from(format!(
                                    "pypy{}.{}",
                                    interp.major, interp.minor
                                )));
                            } else {
                                interps.push(interp);
                            }
                        }
                        // cross compiling to PyPy with abi3 feature enabled,
                        // we cannot use host pypy so switch to bundled sysconfig instead
                        if !pypys.is_empty() {
                            interps.extend(find_interpreter_in_sysconfig(
                                bridge,
                                &pypys,
                                target,
                                requires_python,
                            )?)
                        }
                        if interps.is_empty() {
                            bail!("Failed to find any python interpreter");
                        }
                        Ok(interps)
                    } else {
                        if found_interpreters.is_empty() {
                            bail!("Failed to find any python interpreter");
                        }
                        Ok(found_interpreters)
                    }
                }
            }
        }
    }

    /// Tries to fill the missing metadata for a BuildContext by querying cargo and python
    #[instrument(skip_all)]
    pub fn into_build_context(self) -> BuildContextBuilder {
        BuildContextBuilder::new(self)
    }
}

#[derive(Debug)]
pub struct BuildContextBuilder {
    build_options: BuildOptions,
    release: bool,
    strip: bool,
    editable: bool,
    sdist_only: bool,
}

impl BuildContextBuilder {
    fn new(build_options: BuildOptions) -> Self {
        Self {
            build_options,
            release: false,
            strip: false,
            editable: false,
            sdist_only: false,
        }
    }

    pub fn release(mut self, release: bool) -> Self {
        self.release = release;
        self
    }

    pub fn strip(mut self, strip: bool) -> Self {
        self.strip = strip;
        self
    }

    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    pub fn sdist_only(mut self, sdist_only: bool) -> Self {
        self.sdist_only = sdist_only;
        self
    }

    pub fn build(self) -> Result<BuildContext> {
        let Self {
            build_options,
            release,
            strip,
            editable,
            sdist_only,
        } = self;
        let ProjectResolver {
            project_layout,
            cargo_toml_path,
            cargo_toml,
            pyproject_toml_path,
            pyproject_toml,
            module_name,
            metadata24,
            mut cargo_options,
            cargo_metadata,
            mut pyproject_toml_maturin_options,
        } = ProjectResolver::resolve(
            build_options.manifest_path.clone(),
            build_options.cargo.clone(),
        )?;
        let pyproject = pyproject_toml.as_ref();

        let bridge = find_bridge(
            &cargo_metadata,
            build_options.bindings.as_deref().or_else(|| {
                pyproject.and_then(|x| {
                    if x.bindings().is_some() {
                        pyproject_toml_maturin_options.push("bindings");
                    }
                    x.bindings()
                })
            }),
        )?;

        if !bridge.is_bin() && project_layout.extension_name.contains('-') {
            bail!(
                "The module name must not contain a minus `-` \
                 (Make sure you have set an appropriate [lib] name or \
                 [tool.maturin] module-name in your pyproject.toml)"
            );
        }

        let mut target_triple = build_options.target.clone();

        let mut universal2 = target_triple.as_deref() == Some("universal2-apple-darwin");
        // Also try to determine universal2 from ARCHFLAGS environment variable
        if target_triple.is_none() {
            if let Ok(arch_flags) = env::var("ARCHFLAGS") {
                let arches: HashSet<&str> = arch_flags
                    .split("-arch")
                    .filter_map(|x| {
                        let x = x.trim();
                        if x.is_empty() {
                            None
                        } else {
                            Some(x)
                        }
                    })
                    .collect();
                match (arches.contains("x86_64"), arches.contains("arm64")) {
                    (true, true) => universal2 = true,
                    (true, false) => target_triple = Some("x86_64-apple-darwin".to_string()),
                    (false, true) => target_triple = Some("aarch64-apple-darwin".to_string()),
                    (false, false) => {}
                }
            };
        }
        if universal2 {
            // Ensure that target_triple is valid. This is necessary to properly
            // infer the platform tags when cross-compiling from Linux.
            target_triple = Some("aarch64-apple-darwin".to_string());
        }

        let mut target = Target::from_target_triple(target_triple)?;
        if !target.user_specified && !universal2 {
            if let Some(interpreter) = build_options.interpreter.first() {
                if let Some(detected_target) =
                    crate::target::detect_arch_from_python(interpreter, &target)
                {
                    target = Target::from_target_triple(Some(detected_target))?;
                }
            }
        }

        let wheel_dir = match build_options.out {
            Some(ref dir) => dir.clone(),
            None => PathBuf::from(&cargo_metadata.target_directory).join("wheels"),
        };

        let generate_import_lib = is_generating_import_lib(&cargo_metadata)?;
        let interpreter = if sdist_only && env::var_os("MATURIN_TEST_PYTHON").is_none() {
            // We don't need a python interpreter to build sdist only
            Vec::new()
        } else {
            resolve_interpreters(
                &build_options,
                &bridge,
                &target,
                metadata24.requires_python.as_ref(),
                generate_import_lib,
            )?
        };

        if cargo_options.args.is_empty() {
            // if not supplied on command line, try pyproject.toml
            let tool_maturin = pyproject.and_then(|p| p.maturin());
            if let Some(args) = tool_maturin.and_then(|x| x.rustc_args.as_ref()) {
                cargo_options.args.extend(args.iter().cloned());
                pyproject_toml_maturin_options.push("rustc-args");
            }
        }

        let strip = pyproject.map(|x| x.strip()).unwrap_or_default() || strip;
        let skip_auditwheel = pyproject.map(|x| x.skip_auditwheel()).unwrap_or_default()
            || build_options.skip_auditwheel;
        let auditwheel = build_options
            .auditwheel
            .or_else(|| pyproject.and_then(|x| x.auditwheel()))
            .unwrap_or(if skip_auditwheel {
                AuditWheelMode::Skip
            } else {
                AuditWheelMode::Repair
            });
        let platform_tags = if build_options.platform_tag.is_empty() {
            #[cfg(feature = "zig")]
            let use_zig = build_options.zig;
            #[cfg(not(feature = "zig"))]
            let use_zig = false;
            let compatibility = pyproject
                .and_then(|x| {
                    if x.compatibility().is_some() {
                        pyproject_toml_maturin_options.push("compatibility");
                    }
                    x.compatibility()
                })
                .or(if use_zig {
                    if target.is_musl_libc() {
                        // Zig bundles musl 1.2
                        Some(PlatformTag::Musllinux { x: 1, y: 2 })
                    } else {
                        // With zig we can compile to any glibc version that we want, so we pick the lowest
                        // one supported by the rust compiler
                        Some(target.get_minimum_manylinux_tag())
                    }
                } else {
                    // Defaults to musllinux_1_2 for musl target if it's not bin bindings
                    if target.is_musl_libc() && !bridge.is_bin() {
                        Some(PlatformTag::Musllinux { x: 1, y: 2 })
                    } else {
                        None
                    }
                });
            if let Some(platform_tag) = compatibility {
                vec![platform_tag]
            } else {
                Vec::new()
            }
        } else {
            build_options.platform_tag
        };

        for platform_tag in &platform_tags {
            if !platform_tag.is_supported() {
                eprintln!("⚠️  Warning: {platform_tag} is unsupported by the Rust compiler.");
            } else if platform_tag.is_musllinux() && !target.is_musl_libc() {
                eprintln!("⚠️  Warning: {target} is not compatible with {platform_tag}.");
            }
        }

        validate_bridge_type(&bridge, &target, &platform_tags)?;

        // linux tag can not be mixed with manylinux and musllinux tags
        if platform_tags.len() > 1 && platform_tags.iter().any(|tag| !tag.is_portable()) {
            bail!("Cannot mix linux and manylinux/musllinux platform tags",);
        }

        if !pyproject_toml_maturin_options.is_empty() {
            eprintln!(
                "📡 Using build options {} from pyproject.toml",
                pyproject_toml_maturin_options.join(", ")
            );
        }

        let target_dir = build_options
            .cargo
            .target_dir
            .clone()
            .unwrap_or_else(|| cargo_metadata.target_directory.clone().into_std_path_buf());

        let config_targets = pyproject.and_then(|x| x.targets());
        let compile_targets =
            filter_cargo_targets(&cargo_metadata, bridge, config_targets.as_deref())?;
        if compile_targets.is_empty() {
            bail!("No Cargo targets to build, please check your bindings configuration in pyproject.toml.");
        }

        let crate_name = cargo_toml.package.name;
        Ok(BuildContext {
            target,
            compile_targets,
            project_layout,
            pyproject_toml_path,
            pyproject_toml,
            metadata24,
            crate_name,
            module_name,
            manifest_path: cargo_toml_path,
            target_dir,
            out: wheel_dir,
            release,
            strip,
            auditwheel,
            #[cfg(feature = "zig")]
            zig: build_options.zig,
            platform_tag: platform_tags,
            interpreter,
            cargo_metadata,
            universal2,
            editable,
            cargo_options,
        })
    }
}

fn resolve_interpreters(
    build_options: &BuildOptions,
    bridge: &BridgeModel,
    target: &Target,
    requires_python: Option<&VersionSpecifiers>,
    generate_import_lib: bool,
) -> Result<Vec<PythonInterpreter>, anyhow::Error> {
    let interpreter = if build_options.find_interpreter {
        // Auto-detect interpreters
        build_options.find_interpreters(
            bridge,
            &[],
            target,
            requires_python,
            generate_import_lib,
        )?
    } else {
        // User given list of interpreters
        let interpreter = if build_options.interpreter.is_empty() && !target.cross_compiling() {
            if cfg!(test) {
                match env::var_os("MATURIN_TEST_PYTHON") {
                    Some(python) => vec![python.into()],
                    None => vec![target.get_python()],
                }
            } else {
                vec![target.get_python()]
            }
        } else {
            // XXX: False positive clippy warning
            #[allow(clippy::redundant_clone)]
            build_options.interpreter.clone()
        };
        build_options.find_interpreters(bridge, &interpreter, target, None, generate_import_lib)?
    };
    Ok(interpreter)
}

/// Checks for bridge/platform type edge cases
fn validate_bridge_type(
    bridge: &BridgeModel,
    target: &Target,
    platform_tags: &[PlatformTag],
) -> Result<()> {
    match bridge {
        BridgeModel::Bin(None) => {
            // Only support two different kind of platform tags when compiling to musl target without any binding crates
            if platform_tags.iter().any(|tag| tag.is_musllinux()) && !target.is_musl_libc() {
                bail!(
                    "Cannot mix musllinux and manylinux platform tags when compiling to {}",
                    target.target_triple()
                );
            }

            #[allow(clippy::comparison_chain)]
            if platform_tags.len() > 2 {
                bail!(
                    "Expected only one or two platform tags but found {}",
                    platform_tags.len()
                );
            } else if platform_tags.len() == 2 {
                // The two platform tags can't be the same kind
                let tag_types = platform_tags
                    .iter()
                    .map(|tag| tag.is_musllinux())
                    .collect::<HashSet<_>>();
                if tag_types.len() == 1 {
                    bail!(
                        "Expected only one platform tag but found {}",
                        platform_tags.len()
                    );
                }
            }
        }
        _ => {
            if platform_tags.len() > 1 {
                bail!(
                    "Expected only one platform tag but found {}",
                    platform_tags.len()
                );
            }
        }
    }
    Ok(())
}

fn filter_cargo_targets(
    cargo_metadata: &Metadata,
    bridge: BridgeModel,
    config_targets: Option<&[crate::pyproject_toml::CargoTarget]>,
) -> Result<Vec<CompileTarget>> {
    let root_pkg = cargo_metadata.root_package().unwrap();
    let resolved_features = cargo_metadata
        .resolve
        .as_ref()
        .and_then(|resolve| resolve.nodes.iter().find(|node| node.id == root_pkg.id))
        .map(|node| node.features.clone())
        .unwrap_or_default();
    let mut targets: Vec<_> = root_pkg
        .targets
        .iter()
        .filter(|target| match bridge {
            BridgeModel::Bin(_) => {
                let is_bin = target.is_bin();
                if target.required_features.is_empty() {
                    is_bin
                } else {
                    // Check all required features are enabled for this bin target
                    is_bin
                        && target
                            .required_features
                            .iter()
                            .all(|f| resolved_features.contains(f))
                }
            }
            _ => target.crate_types.contains(&CrateType::CDyLib),
        })
        .map(|target| CompileTarget {
            target: target.clone(),
            bridge_model: bridge.clone(),
        })
        .collect();
    if targets.is_empty() && !bridge.is_bin() {
        // No `crate-type = ["cdylib"]` in `Cargo.toml`
        // Let's try compile one of the target with `--crate-type cdylib`
        let lib_target = root_pkg.targets.iter().find(|target| {
            target
                .crate_types
                .iter()
                .any(|crate_type| LIB_CRATE_TYPES.contains(crate_type))
        });
        if let Some(target) = lib_target {
            targets.push(CompileTarget {
                target: target.clone(),
                bridge_model: bridge,
            });
        }
    }

    // Filter targets by config_targets
    if let Some(config_targets) = config_targets {
        targets.retain(|CompileTarget { target, .. }| {
            config_targets.iter().any(|config_target| {
                let name_eq = config_target.name == target.name;
                match &config_target.kind {
                    Some(kind) => name_eq && target.crate_types.contains(&CrateType::from(*kind)),
                    None => name_eq,
                }
            })
        });
        if targets.is_empty() {
            bail!(
                "No Cargo targets matched by `package.metadata.maturin.targets`, please check your `Cargo.toml`"
            );
        } else {
            let target_names = targets
                .iter()
                .map(|CompileTarget { target, .. }| target.name.as_str())
                .collect::<Vec<_>>();
            eprintln!(
                "🎯 Found {} Cargo targets in `Cargo.toml`: {}",
                targets.len(),
                target_names.join(", ")
            );
        }
    }

    Ok(targets)
}

/// pyo3 supports building abi3 wheels if the unstable-api feature is not selected
fn has_abi3(deps: &HashMap<&str, &Node>) -> Result<Option<(u8, u8)>> {
    for &lib in PYO3_BINDING_CRATES.iter() {
        if let Some(pyo3_crate) = deps.get(lib) {
            // Find the minimal abi3 python version. If there is none, abi3 hasn't been selected
            // This parser abi3-py{major}{minor} and returns the minimal (major, minor) tuple
            let abi3_selected = pyo3_crate.features.iter().any(|x| x == "abi3");

            let min_abi3_version = pyo3_crate
                .features
                .iter()
                .filter(|x| x.starts_with("abi3-py") && x.len() >= "abi3-pyxx".len())
                .map(|x| {
                    Ok((
                        (x.as_bytes()[7] as char).to_string().parse::<u8>()?,
                        x[8..].parse::<u8>()?,
                    ))
                })
                .collect::<Result<Vec<(u8, u8)>>>()
                .context(format!("Bogus {lib} cargo features"))?
                .into_iter()
                .min();
            if abi3_selected && min_abi3_version.is_none() {
                bail!(
                        "You have selected the `abi3` feature but not a minimum version (e.g. the `abi3-py36` feature). \
                        maturin needs a minimum version feature to build abi3 wheels."
                    )
            }
            return Ok(min_abi3_version);
        }
    }
    Ok(None)
}

/// pyo3 0.16.4+ supports building abi3 wheels without a working Python interpreter for Windows
/// when `generate-import-lib` feature is enabled
fn is_generating_import_lib(cargo_metadata: &Metadata) -> Result<bool> {
    let resolve = cargo_metadata
        .resolve
        .as_ref()
        .context("Expected cargo to return metadata with resolve")?;
    for &lib in PYO3_BINDING_CRATES.iter().rev() {
        let pyo3_packages = resolve
            .nodes
            .iter()
            .filter(|package| cargo_metadata[&package.id].name.as_str() == lib)
            .collect::<Vec<_>>();
        match pyo3_packages.as_slice() {
            [pyo3_crate] => {
                let generate_import_lib = pyo3_crate
                    .features
                    .iter()
                    .any(|x| x == "generate-import-lib" || x == "generate-abi3-import-lib");
                return Ok(generate_import_lib);
            }
            _ => continue,
        }
    }
    Ok(false)
}

/// Tries to determine the bindings type from dependency
fn find_bindings(
    deps: &HashMap<&str, &Node>,
    packages: &HashMap<&str, &cargo_metadata::Package>,
) -> Option<Bindings> {
    if deps.get("pyo3").is_some() {
        let version = packages["pyo3"].version.clone();
        Some(Bindings {
            name: "pyo3".to_string(),
            version,
        })
    } else if deps.get("pyo3-ffi").is_some() {
        let version = packages["pyo3-ffi"].version.clone();
        Some(Bindings {
            name: "pyo3-ffi".to_string(),
            version,
        })
    } else if deps.contains_key("uniffi") {
        let version = packages["uniffi"].version.clone();
        Some(Bindings {
            name: "uniffi".to_string(),
            version,
        })
    } else {
        None
    }
}

/// Return a map with all (transitive) dependencies of the *current* crate.
/// This is different from `metadata.resolve`, which also includes packages
/// that are used in the same workspace, but on which the current crate does not depend.
fn current_crate_dependencies(cargo_metadata: &Metadata) -> Result<HashMap<&str, &Node>> {
    let resolve = cargo_metadata
        .resolve
        .as_ref()
        .context("Expected to get a dependency graph from cargo")?;
    let root = resolve
        .root
        .as_ref()
        .context("expected to get a root package")?;
    let nodes: HashMap<&PackageId, &Node> =
        resolve.nodes.iter().map(|node| (&node.id, node)).collect();

    // Walk the dependency tree to get all (in)direct children.
    let mut dep_ids = HashSet::with_capacity(nodes.len());
    let mut todo = Vec::from([root]);
    while let Some(id) = todo.pop() {
        for dep in nodes[id].deps.iter() {
            if dep_ids.contains(&dep.pkg) {
                continue;
            }
            dep_ids.insert(&dep.pkg);
            todo.push(&dep.pkg);
        }
    }

    Ok(nodes
        .into_iter()
        .filter_map(|(id, node)| {
            dep_ids
                .contains(&id)
                .then_some((cargo_metadata[id].name.as_ref(), node))
        })
        .collect())
}

/// Tries to determine the [BridgeModel] for the target crate
pub fn find_bridge(cargo_metadata: &Metadata, bridge: Option<&str>) -> Result<BridgeModel> {
    let deps = current_crate_dependencies(cargo_metadata)?;
    let packages: HashMap<&str, &cargo_metadata::Package> = cargo_metadata
        .packages
        .iter()
        .filter_map(|pkg| {
            let name = &pkg.name;
            if name == "pyo3" || name == "pyo3-ffi" || name == "cpython" || name == "uniffi" {
                Some((name.as_ref(), pkg))
            } else {
                None
            }
        })
        .collect();
    let root_package = cargo_metadata
        .root_package()
        .context("Expected cargo to return metadata with root_package")?;
    let targets: Vec<_> = root_package
        .targets
        .iter()
        .filter(|target| {
            target.kind.iter().any(|kind| {
                !matches!(
                    kind,
                    TargetKind::Bench
                        | TargetKind::CustomBuild
                        | TargetKind::Example
                        | TargetKind::ProcMacro
                        | TargetKind::Test
                )
            })
        })
        .flat_map(|target| target.crate_types.iter().cloned())
        .collect();

    let bridge = if let Some(bindings) = bridge {
        if bindings == "cffi" {
            BridgeModel::Cffi
        } else if bindings == "uniffi" {
            BridgeModel::UniFfi
        } else if bindings == "bin" {
            // uniffi bindings don't support bin
            let bindings =
                find_bindings(&deps, &packages).filter(|bindings| bindings.name != "uniffi");
            BridgeModel::Bin(bindings)
        } else {
            if !deps.contains_key(bindings) {
                bail!(
                    "The bindings crate {} was not found in the dependencies list",
                    bindings
                );
            }

            let version = packages[bindings].version.clone();
            BridgeModel::Bindings(Bindings {
                name: bindings.to_string(),
                version,
            })
        }
    } else if let Some(bindings) = find_bindings(&deps, &packages) {
        if !targets.contains(&CrateType::CDyLib) && targets.contains(&CrateType::Bin) {
            if bindings.name == "uniffi" {
                // uniffi bindings don't support bin
                BridgeModel::Bin(None)
            } else {
                BridgeModel::Bin(Some(bindings))
            }
        } else if bindings.name == "uniffi" {
            BridgeModel::UniFfi
        } else {
            BridgeModel::Bindings(bindings)
        }
    } else if targets.contains(&CrateType::CDyLib) {
        BridgeModel::Cffi
    } else if targets.contains(&CrateType::Bin) {
        BridgeModel::Bin(find_bindings(&deps, &packages))
    } else {
        bail!("Couldn't detect the binding type; Please specify them with --bindings/-b")
    };

    if !(bridge.is_bindings("pyo3") || bridge.is_bindings("pyo3-ffi")) {
        eprintln!("🔗 Found {bridge} bindings");
        return Ok(bridge);
    }

    for &lib in PYO3_BINDING_CRATES.iter() {
        if !bridge.is_bin() && bridge.is_bindings(lib) {
            let pyo3_node = deps[lib];
            if !pyo3_node.features.contains(&"extension-module".to_string()) {
                let version = cargo_metadata[&pyo3_node.id].version.to_string();
                eprintln!(
                    "⚠️  Warning: You're building a library without activating {lib}'s \
                     `extension-module` feature. \
                     See https://pyo3.rs/v{version}/building-and-distribution.html#the-extension-module-feature"
                );
            }

            return if let Some((major, minor)) = has_abi3(&deps)? {
                eprintln!("🔗 Found {lib} bindings with abi3 support for Python ≥ {major}.{minor}");
                let version = packages[lib].version.clone();
                let bindings = Bindings {
                    name: lib.to_string(),
                    version,
                };
                Ok(BridgeModel::BindingsAbi3 {
                    bindings,
                    major,
                    minor,
                })
            } else {
                eprintln!("🔗 Found {lib} bindings");
                Ok(bridge)
            };
        }
    }

    Ok(bridge)
}

/// Shared between cffi and pyo3-abi3
fn find_single_python_interpreter(
    bridge: &BridgeModel,
    interpreter: &[PathBuf],
    target: &Target,
    bridge_name: &str,
) -> Result<PythonInterpreter> {
    let err_message = "Failed to find a python interpreter";

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
        .context(format_err!(err_message))?
        .ok_or_else(|| format_err!(err_message))?;
    Ok(interpreter)
}

/// Find python interpreters in host machine first,
/// fallback to bundled sysconfig if not found in host machine
fn find_interpreter(
    bridge: &BridgeModel,
    interpreter: &[PathBuf],
    target: &Target,
    requires_python: Option<&VersionSpecifiers>,
) -> Result<Vec<PythonInterpreter>> {
    let mut found_interpreters = Vec::new();
    if !interpreter.is_empty() {
        let mut missing = Vec::new();
        for interp in interpreter {
            match PythonInterpreter::check_executable(interp.clone(), target, bridge) {
                Ok(Some(interp)) => found_interpreters.push(interp),
                _ => missing.push(interp.clone()),
            }
        }
        if !missing.is_empty() {
            let sysconfig_interps =
                find_interpreter_in_sysconfig(bridge, &missing, target, requires_python)?;
            found_interpreters.extend(sysconfig_interps);
        }
    } else {
        found_interpreters = PythonInterpreter::find_all(target, bridge, requires_python)
            .context("Finding python interpreters failed")?;
    };

    if found_interpreters.is_empty() {
        if interpreter.is_empty() {
            if let Some(requires_python) = requires_python {
                bail!("Couldn't find any python interpreters with version {}. Please specify at least one with -i", requires_python);
            } else {
                bail!("Couldn't find any python interpreters. Please specify at least one with -i");
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

/// Find python interpreters in the host machine
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
            bail!("Couldn't find any python interpreters with {}. Please specify at least one with -i", requires_python);
        } else {
            bail!("Couldn't find any python interpreters. Please specify at least one with -i");
        }
    }
    Ok(interpreters)
}

/// Find python interpreters in the bundled sysconfig
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
            // Also accept things like `python3.13t` for free-threaded python
            let (ver, abiflags) = maybe_free_threaded(ver.strip_prefix('-').unwrap_or(ver));
            (InterpreterKind::CPython, ver, abiflags)
        } else if python
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            // Eg: -i 3.9 without interpreter kind, assume it's CPython
            let (ver, abiflags) = maybe_free_threaded(&python);
            (InterpreterKind::CPython, ver, abiflags)
        } else {
            // if interpreter not known
            if std::path::Path::new(&python).is_file() {
                bail!("Python interpreter should be a kind of interpreter (e.g. 'python3.8' or 'pypy3.9') when cross-compiling, got path to interpreter: {}", python);
            } else {
                bail!("Unsupported Python interpreter for cross-compilation: {}; supported interpreters are pypy, graalpy, and python (cpython)", python);
            }
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

        let sysconfig = InterpreterConfig::lookup_one(target, python_impl, (ver_major, ver_minor), abiflags)
            .with_context(|| {
                format!("Failed to find a {python_impl} {ver_major}.{ver_minor} interpreter in known sysconfig")
            })?;
        debug!(
            "Found {} {}.{} in bundled sysconfig",
            sysconfig.interpreter_kind, sysconfig.major, sysconfig.minor,
        );
        interpreters.push(PythonInterpreter::from_config(sysconfig.clone()));
    }
    Ok(interpreters)
}

fn maybe_free_threaded(python_ver: &str) -> (&str, &str) {
    if let Some(ver) = python_ver.strip_suffix('t') {
        (ver, "t")
    } else {
        (python_ver, "")
    }
}

/// We need to pass the global flags to cargo metadata
/// (https://github.com/PyO3/maturin/issues/211 and https://github.com/PyO3/maturin/issues/472),
/// but we can't pass all the extra args, as e.g. `--target` isn't supported, so this tries to
/// extract the arguments for cargo metadata
///
/// There are flags (without value) and options (with value). The options value be passed
/// in the same string as its name or in the next one. For this naive parsing logic, we
/// assume that the value is in the next argument if the argument string equals the name,
/// otherwise it's in the same argument and the next argument is unrelated.
pub(crate) fn extract_cargo_metadata_args(cargo_options: &CargoOptions) -> Result<Vec<String>> {
    let mut cargo_metadata_extra_args = vec![];
    if cargo_options.frozen {
        cargo_metadata_extra_args.push("--frozen".to_string());
    }
    if cargo_options.locked {
        cargo_metadata_extra_args.push("--locked".to_string());
    }
    if cargo_options.offline {
        cargo_metadata_extra_args.push("--offline".to_string());
    }
    for feature in &cargo_options.features {
        cargo_metadata_extra_args.push("--features".to_string());
        cargo_metadata_extra_args.push(feature.clone());
    }
    if cargo_options.all_features {
        cargo_metadata_extra_args.push("--all-features".to_string());
    }
    if cargo_options.no_default_features {
        cargo_metadata_extra_args.push("--no-default-features".to_string());
    }
    for opt in &cargo_options.unstable_flags {
        cargo_metadata_extra_args.push("-Z".to_string());
        cargo_metadata_extra_args.push(opt.clone());
    }
    Ok(cargo_metadata_extra_args)
}

impl From<CargoOptions> for cargo_options::Rustc {
    fn from(cargo: CargoOptions) -> Self {
        cargo_options::Rustc {
            common: cargo_options::CommonOptions {
                quiet: cargo.quiet,
                jobs: cargo.jobs,
                profile: cargo.profile,
                features: cargo.features,
                all_features: cargo.all_features,
                no_default_features: cargo.no_default_features,
                target: match cargo.target {
                    Some(target) => vec![target],
                    None => Vec::new(),
                },
                target_dir: cargo.target_dir,
                verbose: cargo.verbose,
                color: cargo.color,
                frozen: cargo.frozen,
                locked: cargo.locked,
                offline: cargo.offline,
                config: cargo.config,
                unstable_flags: cargo.unstable_flags,
                timings: cargo.timings,
                ..Default::default()
            },
            manifest_path: cargo.manifest_path,
            ignore_rust_version: cargo.ignore_rust_version,
            future_incompat_report: cargo.future_incompat_report,
            args: cargo.args,
            ..Default::default()
        }
    }
}

impl CargoOptions {
    /// Merge options from pyproject.toml
    pub fn merge_with_pyproject_toml(&mut self, tool_maturin: ToolMaturin) -> Vec<&'static str> {
        let mut args_from_pyproject = Vec::new();

        if self.manifest_path.is_none() && tool_maturin.manifest_path.is_some() {
            self.manifest_path.clone_from(&tool_maturin.manifest_path);
            args_from_pyproject.push("manifest-path");
        }

        if self.profile.is_none() && tool_maturin.profile.is_some() {
            self.profile.clone_from(&tool_maturin.profile);
            args_from_pyproject.push("profile");
        }

        if let Some(features) = tool_maturin.features {
            if self.features.is_empty() {
                self.features = features;
                args_from_pyproject.push("features");
            }
        }

        if let Some(all_features) = tool_maturin.all_features {
            if !self.all_features {
                self.all_features = all_features;
                args_from_pyproject.push("all-features");
            }
        }

        if let Some(no_default_features) = tool_maturin.no_default_features {
            if !self.no_default_features {
                self.no_default_features = no_default_features;
                args_from_pyproject.push("no-default-features");
            }
        }

        if let Some(frozen) = tool_maturin.frozen {
            if !self.frozen {
                self.frozen = frozen;
                args_from_pyproject.push("frozen");
            }
        }

        if let Some(locked) = tool_maturin.locked {
            if !self.locked {
                self.locked = locked;
                args_from_pyproject.push("locked");
            }
        }

        if let Some(config) = tool_maturin.config {
            if self.config.is_empty() {
                self.config = config;
                args_from_pyproject.push("config");
            }
        }

        if let Some(unstable_flags) = tool_maturin.unstable_flags {
            if self.unstable_flags.is_empty() {
                self.unstable_flags = unstable_flags;
                args_from_pyproject.push("unstable-flags");
            }
        }

        args_from_pyproject
    }
}

#[cfg(test)]
mod test {
    use cargo_metadata::MetadataCommand;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    use super::*;

    #[test]
    fn test_find_bridge_pyo3() {
        let pyo3_mixed = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/pyo3-mixed").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_mixed, None),
            Ok(BridgeModel::Bindings { .. })
        ));
        assert!(matches!(
            find_bridge(&pyo3_mixed, Some("pyo3")),
            Ok(BridgeModel::Bindings { .. })
        ));
    }

    #[test]
    fn test_find_bridge_pyo3_abi3() {
        let pyo3_pure = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/pyo3-pure").join("Cargo.toml"))
            .exec()
            .unwrap();

        let bridge = BridgeModel::BindingsAbi3 {
            bindings: Bindings {
                name: "pyo3".to_string(),
                version: semver::Version::new(0, 24, 0),
            },
            major: 3,
            minor: 7,
        };
        assert_eq!(find_bridge(&pyo3_pure, None).unwrap(), bridge);
        assert_eq!(find_bridge(&pyo3_pure, Some("pyo3")).unwrap(), bridge);
    }

    #[test]
    fn test_find_bridge_pyo3_feature() {
        let pyo3_pure = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/pyo3-feature").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(find_bridge(&pyo3_pure, None).is_err());

        let pyo3_pure = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/pyo3-feature").join("Cargo.toml"))
            .other_options(vec!["--features=pyo3".to_string()])
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_pure, None).unwrap(),
            BridgeModel::Bindings { .. }
        ));
    }

    #[test]
    fn test_find_bridge_cffi() {
        let cffi_pure = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/cffi-pure").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert_eq!(
            find_bridge(&cffi_pure, Some("cffi")).unwrap(),
            BridgeModel::Cffi
        );
        assert_eq!(find_bridge(&cffi_pure, None).unwrap(), BridgeModel::Cffi);

        assert!(find_bridge(&cffi_pure, Some("pyo3")).is_err());
    }

    #[test]
    fn test_find_bridge_bin() {
        let hello_world = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/hello-world").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert_eq!(
            find_bridge(&hello_world, Some("bin")).unwrap(),
            BridgeModel::Bin(None)
        );
        assert_eq!(
            find_bridge(&hello_world, None).unwrap(),
            BridgeModel::Bin(None)
        );

        assert!(find_bridge(&hello_world, Some("pyo3")).is_err());

        let pyo3_bin = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/pyo3-bin").join("Cargo.toml"))
            .exec()
            .unwrap();
        assert!(matches!(
            find_bridge(&pyo3_bin, Some("bin")).unwrap(),
            BridgeModel::Bin(Some(_))
        ));
        assert!(matches!(
            find_bridge(&pyo3_bin, None).unwrap(),
            BridgeModel::Bin(Some(_))
        ));
    }

    #[test]
    fn test_old_extra_feature_args() {
        let cargo_extra_args = CargoOptions {
            no_default_features: true,
            features: vec!["a".to_string(), "c".to_string()],
            target: Some("x86_64-unknown-linux-musl".to_string()),
            ..Default::default()
        };
        let cargo_metadata_extra_args = extract_cargo_metadata_args(&cargo_extra_args).unwrap();
        assert_eq!(
            cargo_metadata_extra_args,
            vec![
                "--features",
                "a",
                "--features",
                "c",
                "--no-default-features",
            ]
        );
    }

    #[test]
    fn test_extract_cargo_metadata_args() {
        let args = CargoOptions {
            locked: true,
            features: vec!["my-feature".to_string(), "other-feature".to_string()],
            target: Some("x86_64-unknown-linux-musl".to_string()),
            unstable_flags: vec!["unstable-options".to_string()],
            ..Default::default()
        };

        let expected = vec![
            "--locked",
            "--features",
            "my-feature",
            "--features",
            "other-feature",
            "-Z",
            "unstable-options",
        ];

        assert_eq!(extract_cargo_metadata_args(&args).unwrap(), expected);
    }
}
