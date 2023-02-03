use crate::auditwheel::PlatformTag;
use crate::build_context::BridgeModel;
use crate::compile::{CompileTarget, LIB_CRATE_TYPES};
use crate::cross_compile::{find_sysconfigdata, parse_sysconfigdata};
use crate::project_layout::ProjectResolver;
use crate::pyproject_toml::ToolMaturin;
use crate::python_interpreter::{InterpreterConfig, InterpreterKind, MINIMUM_PYTHON_MINOR};
use crate::{BuildContext, Metadata21, PythonInterpreter, Target};
use anyhow::{bail, format_err, Context, Result};
use cargo_metadata::{Metadata, Node};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use tracing::debug;

// This is used for BridgeModel::Bindings("pyo3-ffi") and BridgeModel::Bindings("pyo3").
// These should be treated almost identically but must be correctly identified
// as one or the other in logs. pyo3-ffi is ordered first because it is newer
// and more restrictive.
const PYO3_BINDING_CRATES: [&str; 2] = ["pyo3-ffi", "pyo3"];

fn pyo3_minimum_python_minor_version(major_version: u64, minor_version: u64) -> Option<usize> {
    if (major_version, minor_version) >= (0, 16) {
        Some(7)
    } else {
        None
    }
}

fn pyo3_ffi_minimum_python_minor_version(major_version: u64, minor_version: u64) -> Option<usize> {
    if (major_version, minor_version) >= (0, 16) {
        pyo3_minimum_python_minor_version(major_version, minor_version)
    } else {
        None
    }
}

/// Cargo options for the build process
#[derive(Debug, Default, Serialize, Deserialize, clap::Parser, Clone, Eq, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
pub struct CargoOptions {
    /// Do not print cargo log messages
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Number of parallel jobs, defaults to # of CPUs
    #[arg(short = 'j', long, value_name = "N")]
    pub jobs: Option<usize>,

    /// Build artifacts with the specified Cargo profile
    #[arg(long, value_name = "PROFILE-NAME")]
    pub profile: Option<String>,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long, action = clap::ArgAction::Append)]
    pub features: Vec<String>,

    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,

    /// Do not activate the `default` feature
    #[arg(long)]
    pub no_default_features: bool,

    /// Build for the target triple
    #[arg(long, value_name = "TRIPLE", env = "CARGO_BUILD_TARGET")]
    pub target: Option<String>,

    /// Directory for all generated artifacts
    #[arg(long, value_name = "DIRECTORY")]
    pub target_dir: Option<PathBuf>,

    /// Path to Cargo.toml
    #[arg(short = 'm', long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Ignore `rust-version` specification in packages
    #[arg(long)]
    pub ignore_rust_version: bool,

    /// Use verbose output (-vv very verbose/build.rs output)
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Coloring: auto, always, never
    #[arg(long, value_name = "WHEN")]
    pub color: Option<String>,

    /// Require Cargo.lock and cache are up to date
    #[arg(long)]
    pub frozen: bool,

    /// Require Cargo.lock is up to date
    #[arg(long)]
    pub locked: bool,

    /// Run without accessing the network
    #[arg(long)]
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
        require_equals = true
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
    #[arg(short, long, value_parser = ["pyo3", "pyo3-ffi", "rust-cpython", "cffi", "uniffi", "bin"])]
    pub bindings: Option<String>,

    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    #[arg(short, long)]
    pub out: Option<PathBuf>,

    /// Don't check for manylinux compliance
    #[arg(long = "skip-auditwheel")]
    pub skip_auditwheel: bool,

    /// For manylinux targets, use zig to ensure compliance for the chosen manylinux version
    ///
    /// Default to manylinux2014/manylinux_2_17 if you do not specify an `--compatibility`
    ///
    /// Make sure you installed zig with `pip install maturin[zig]`
    #[cfg(feature = "zig")]
    #[arg(long)]
    pub zig: bool,

    /// Control whether to build universal2 wheel for macOS or not.
    /// Only applies to macOS targets, do nothing otherwise.
    #[arg(long)]
    pub universal2: bool,

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
        min_python_minor: Option<usize>,
        generate_import_lib: bool,
    ) -> Result<Vec<PythonInterpreter>> {
        match bridge {
            BridgeModel::Bindings(binding_name, _) | BridgeModel::Bin(Some((binding_name, _))) => {
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
                        let host_interpreters = find_interpreter_in_host(
                            bridge,
                            interpreter,
                            target,
                            min_python_minor,
                        )?;
                        let host_python = &host_interpreters[0];
                        println!("🐍 Using host {host_python} for cross-compiling preparation");
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
                        let ext_suffix = sysconfig_data
                            .get("EXT_SUFFIX")
                            .context("syconfig didn't define an `EXT_SUFFIX` ಠ_ಠ")?;
                        let soabi = sysconfig_data.get("SOABI");
                        let abi_tag =
                            soabi.and_then(|abi| abi.split('-').nth(1).map(ToString::to_string));
                        let interpreter_kind = soabi
                            .and_then(|tag| {
                                if tag.starts_with("pypy") {
                                    Some(InterpreterKind::PyPy)
                                } else if tag.starts_with("cpython") {
                                    Some(InterpreterKind::CPython)
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
                                abi_tag,
                                pointer_width: None,
                            },
                            executable: PathBuf::new(),
                            platform: None,
                            runnable: false,
                            implmentation_name: interpreter_kind.to_string().to_ascii_lowercase(),
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
                        interpreters =
                            find_interpreter_in_sysconfig(interpreter, target, min_python_minor)?;
                    }
                } else if binding_name.starts_with("pyo3") {
                    // Only pyo3/pyo3-ffi bindings supports bundled sysconfig interpreters
                    interpreters = find_interpreter(bridge, interpreter, target, min_python_minor)?;
                } else {
                    interpreters =
                        find_interpreter_in_host(bridge, interpreter, target, min_python_minor)?;
                }

                let interpreters_str = interpreters
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<String>>()
                    .join(", ");
                println!("🐍 Found {interpreters_str}");

                Ok(interpreters)
            }
            BridgeModel::Cffi => {
                let interpreter =
                    find_single_python_interpreter(bridge, interpreter, target, "cffi")?;
                println!("🐍 Using {interpreter} to generate the cffi bindings");
                Ok(vec![interpreter])
            }
            BridgeModel::Bin(None) | BridgeModel::UniFfi => Ok(vec![]),
            BridgeModel::BindingsAbi3(major, minor) => {
                if target.is_windows() {
                    // Ideally, we wouldn't want to use any python interpreter without abi3 at all.
                    // Unfortunately, on windows we need one to figure out base_prefix for a linker
                    // argument.
                    let interpreters = find_interpreter_in_host(
                        bridge,
                        interpreter,
                        target,
                        Some(*minor as usize),
                    )
                    .unwrap_or_default();
                    if env::var_os("PYO3_CROSS_LIB_DIR").is_some() {
                        // PYO3_CROSS_LIB_DIR should point to the `libs` directory inside base_prefix
                        // when cross compiling, so we fake a python interpreter matching it
                        println!("⚠️  Cross-compiling is poorly supported");
                        Ok(vec![PythonInterpreter {
                            config: InterpreterConfig {
                                major: *major as usize,
                                minor: *minor as usize,
                                interpreter_kind: InterpreterKind::CPython,
                                abiflags: "".to_string(),
                                ext_suffix: ".pyd".to_string(),
                                abi_tag: None,
                                pointer_width: None,
                            },
                            executable: PathBuf::new(),
                            platform: None,
                            runnable: false,
                            implmentation_name: "cpython".to_string(),
                            soabi: None,
                        }])
                    } else if let Some(interp) = interpreters.get(0) {
                        println!("🐍 Using {interp} to generate to link bindings (With abi3, an interpreter is only required on windows)");
                        Ok(interpreters)
                    } else if generate_import_lib {
                        println!("🐍 Not using a specific python interpreter (Automatically generating windows import library)");
                        // fake a python interpreter
                        Ok(vec![PythonInterpreter {
                            config: InterpreterConfig {
                                major: *major as usize,
                                minor: *minor as usize,
                                interpreter_kind: InterpreterKind::CPython,
                                abiflags: "".to_string(),
                                ext_suffix: ".pyd".to_string(),
                                abi_tag: None,
                                pointer_width: None,
                            },
                            executable: PathBuf::new(),
                            platform: None,
                            runnable: false,
                            implmentation_name: "cpython".to_string(),
                            soabi: None,
                        }])
                    } else {
                        bail!("Failed to find a python interpreter");
                    }
                } else {
                    let found_interpreters = find_interpreter_in_host(
                        bridge,
                        interpreter,
                        target,
                        Some(*minor as usize),
                    )
                    .or_else(|err| {
                        let interps = find_interpreter_in_sysconfig(
                            interpreter,
                            target,
                            Some(*minor as usize),
                        )
                        .unwrap_or_default();
                        if interps.is_empty() && !self.interpreter.is_empty() {
                            // Print error when user supplied `--interpreter` option
                            Err(err)
                        } else {
                            Ok(interps)
                        }
                    })?;
                    println!("🐍 Not using a specific python interpreter");
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
                                abi_tag: None,
                                pointer_width: None,
                            },
                            executable: PathBuf::new(),
                            platform: None,
                            runnable: false,
                            implmentation_name: "cpython".to_string(),
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
                                &pypys,
                                target,
                                min_python_minor,
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
    pub fn into_build_context(
        self,
        release: bool,
        strip: bool,
        editable: bool,
    ) -> Result<BuildContext> {
        let ProjectResolver {
            project_layout,
            cargo_toml_path,
            cargo_toml,
            pyproject_toml_path,
            pyproject_toml,
            module_name,
            metadata21,
            mut cargo_options,
            cargo_metadata,
            mut pyproject_toml_maturin_options,
        } = ProjectResolver::resolve(self.manifest_path.clone(), self.cargo.clone())?;
        let pyproject = pyproject_toml.as_ref();

        let bridge = find_bridge(
            &cargo_metadata,
            self.bindings.as_deref().or_else(|| {
                pyproject.and_then(|x| {
                    if x.bindings().is_some() {
                        pyproject_toml_maturin_options.push("bindings");
                    }
                    x.bindings()
                })
            }),
        )?;

        if !bridge.is_bin() && module_name.contains('-') {
            bail!(
                "The module name must not contain a minus `-` \
                 (Make sure you have set an appropriate [lib] name or \
                 [package.metadata.maturin] name in your Cargo.toml)"
            );
        }

        let mut target_triple = self.target.clone();

        let mut universal2 = self.universal2;
        if universal2 {
            eprintln!("⚠️  Warning: `--universal2` is deprecated, use `--target univeral2-apple-darwin` instead");
        } else if target_triple.as_deref() == Some("universal2-apple-darwin") {
            universal2 = true;
            target_triple = None;
        }
        // Also try to determine universal2 from ARCHFLAGS environment variable
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
                (true, false) if target_triple.is_none() => {
                    target_triple = Some("x86_64-apple-darwin".to_string())
                }
                (false, true) if target_triple.is_none() => {
                    target_triple = Some("aarch64-apple-darwin".to_string())
                }
                _ => {}
            }
        };

        let target = Target::from_target_triple(target_triple)?;

        let wheel_dir = match self.out {
            Some(ref dir) => dir.clone(),
            None => PathBuf::from(&cargo_metadata.target_directory).join("wheels"),
        };

        let generate_import_lib = is_generating_import_lib(&cargo_metadata)?;
        let interpreter = if self.find_interpreter {
            // Auto-detect interpreters
            self.find_interpreters(
                &bridge,
                &[],
                &target,
                get_min_python_minor(&metadata21),
                generate_import_lib,
            )?
        } else {
            // User given list of interpreters
            let interpreter = if self.interpreter.is_empty() && !target.cross_compiling() {
                if cfg!(test) {
                    match env::var_os("MATURIN_TEST_PYTHON") {
                        Some(python) => vec![python.into()],
                        None => vec![PathBuf::from("python3")],
                    }
                } else {
                    vec![PathBuf::from("python3")]
                }
            } else {
                self.interpreter.clone()
            };
            self.find_interpreters(&bridge, &interpreter, &target, None, generate_import_lib)?
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
        let skip_auditwheel =
            pyproject.map(|x| x.skip_auditwheel()).unwrap_or_default() || self.skip_auditwheel;
        let platform_tags = if self.platform_tag.is_empty() {
            #[cfg(feature = "zig")]
            let use_zig = self.zig;
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
                    if target.is_musl_target() {
                        // Zig bundles musl 1.2
                        Some(PlatformTag::Musllinux { x: 1, y: 2 })
                    } else {
                        // With zig we can compile to any glibc version that we want, so we pick the lowest
                        // one supported by the rust compiler
                        Some(target.get_minimum_manylinux_tag())
                    }
                } else {
                    // Defaults to musllinux_1_2 for musl target if it's not bin bindings
                    if target.is_musl_target() && !bridge.is_bin() {
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
            self.platform_tag
        };

        for platform_tag in &platform_tags {
            if !platform_tag.is_supported() {
                eprintln!("⚠️  Warning: {platform_tag} is unsupported by the Rust compiler.");
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

        let target_dir = self
            .cargo
            .target_dir
            .clone()
            .unwrap_or_else(|| cargo_metadata.target_directory.clone().into_std_path_buf());

        let remaining_core_metadata = cargo_toml.remaining_core_metadata();
        let config_targets = remaining_core_metadata.targets.as_deref();
        let cargo_targets = filter_cargo_targets(&cargo_metadata, bridge, config_targets)?;

        let crate_name = cargo_toml.package.name;
        Ok(BuildContext {
            target,
            cargo_targets,
            project_layout,
            pyproject_toml_path,
            pyproject_toml,
            metadata21,
            crate_name,
            module_name,
            manifest_path: cargo_toml_path,
            target_dir,
            out: wheel_dir,
            release,
            strip,
            skip_auditwheel,
            #[cfg(feature = "zig")]
            zig: self.zig,
            platform_tag: platform_tags,
            interpreter,
            cargo_metadata,
            universal2,
            editable,
            cargo_options,
        })
    }
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
            if platform_tags.iter().any(|tag| tag.is_musllinux()) && !target.is_musl_target() {
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
    config_targets: Option<&[crate::cargo_toml::CargoTarget]>,
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
                let is_bin = target.kind.contains(&"bin".to_string());
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
            _ => target.kind.contains(&"cdylib".to_string()),
        })
        .map(|target| (target.clone(), bridge.clone()))
        .collect();
    if targets.is_empty() && !bridge.is_bin() {
        // No `crate-type = ["cdylib"]` in `Cargo.toml`
        // Let's try compile one of the target with `--crate-type cdylib`
        let lib_target = root_pkg.targets.iter().find(|target| {
            target
                .kind
                .iter()
                .any(|k| LIB_CRATE_TYPES.contains(&k.as_str()))
        });
        if let Some(target) = lib_target {
            targets.push((target.clone(), bridge));
        }
    }

    // Filter targets by config_targets
    if let Some(config_targets) = config_targets {
        targets.retain(|(target, _)| {
            config_targets.iter().any(|config_target| {
                let name_eq = config_target.name == target.name;
                match &config_target.kind {
                    Some(kind) => name_eq && target.kind.contains(kind),
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
                .map(|(target, _)| target.name.as_str())
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

/// Uses very simple PEP 440 subset parsing to determine the
/// minimum supported python minor version for interpreter search
fn get_min_python_minor(metadata21: &Metadata21) -> Option<usize> {
    if let Some(requires_python) = &metadata21.requires_python {
        let regex = Regex::new(r#">=3\.(\d+)(?:\.\d)?"#).unwrap();
        if let Some(captures) = regex.captures(requires_python) {
            let min_python_minor = captures[1]
                .parse::<usize>()
                .expect("Regex must only match usize");
            Some(min_python_minor)
        } else {
            println!(
                "⚠️ Couldn't parse the value of requires-python, \
                    not taking it into account when searching for python interpreter. \
                    Note: Only `>=3.x.y` is currently supported."
            );
            None
        }
    } else {
        None
    }
}

/// pyo3 supports building abi3 wheels if the unstable-api feature is not selected
fn has_abi3(cargo_metadata: &Metadata) -> Result<Option<(u8, u8)>> {
    let resolve = cargo_metadata
        .resolve
        .as_ref()
        .context("Expected cargo to return metadata with resolve")?;
    for &lib in PYO3_BINDING_CRATES.iter() {
        let pyo3_packages = resolve
            .nodes
            .iter()
            .filter(|package| cargo_metadata[&package.id].name.as_str() == lib)
            .collect::<Vec<_>>();
        match pyo3_packages.as_slice() {
            [pyo3_crate] => {
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
            _ => continue,
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
) -> Option<(String, usize)> {
    if deps.get("pyo3").is_some() {
        let ver = &packages["pyo3"].version;
        let minor =
            pyo3_minimum_python_minor_version(ver.major, ver.minor).unwrap_or(MINIMUM_PYTHON_MINOR);
        Some(("pyo3".to_string(), minor))
    } else if deps.get("pyo3-ffi").is_some() {
        let ver = &packages["pyo3-ffi"].version;
        let minor = pyo3_ffi_minimum_python_minor_version(ver.major, ver.minor)
            .unwrap_or(MINIMUM_PYTHON_MINOR);
        Some(("pyo3-ffi".to_string(), minor))
    } else if deps.contains_key("cpython") {
        Some(("rust-cpython".to_string(), MINIMUM_PYTHON_MINOR))
    } else if deps.contains_key("uniffi") {
        Some(("uniffi".to_string(), MINIMUM_PYTHON_MINOR))
    } else {
        None
    }
}

/// Tries to determine the [BridgeModel] for the target crate
pub fn find_bridge(cargo_metadata: &Metadata, bridge: Option<&str>) -> Result<BridgeModel> {
    let resolve = cargo_metadata
        .resolve
        .as_ref()
        .ok_or_else(|| format_err!("Expected to get a dependency graph from cargo"))?;

    let deps: HashMap<&str, &Node> = resolve
        .nodes
        .iter()
        .map(|node| (cargo_metadata[&node.id].name.as_ref(), node))
        .collect();
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
                kind != "example" && kind != "test" && kind != "bench" && kind != "custom-build"
            })
        })
        .flat_map(|target| target.crate_types.iter())
        .map(String::as_str)
        .collect();

    let bridge = if let Some(bindings) = bridge {
        if bindings == "cffi" {
            BridgeModel::Cffi
        } else if bindings == "uniffi" {
            BridgeModel::UniFfi
        } else if bindings == "bin" {
            // uniffi bindings don't support bin
            let bindings =
                find_bindings(&deps, &packages).filter(|(bindings, _)| bindings != "uniffi");
            BridgeModel::Bin(bindings)
        } else {
            if !deps.contains_key(bindings) {
                bail!(
                    "The bindings crate {} was not found in the dependencies list",
                    bindings
                );
            }

            BridgeModel::Bindings(bindings.to_string(), MINIMUM_PYTHON_MINOR)
        }
    } else if let Some((bindings, minor)) = find_bindings(&deps, &packages) {
        if !targets.contains(&"cdylib") && targets.contains(&"bin") {
            if bindings == "uniffi" {
                // uniffi bindings don't support bin
                BridgeModel::Bin(None)
            } else {
                BridgeModel::Bin(Some((bindings, minor)))
            }
        } else if bindings == "uniffi" {
            BridgeModel::UniFfi
        } else {
            BridgeModel::Bindings(bindings, minor)
        }
    } else if targets.contains(&"cdylib") {
        BridgeModel::Cffi
    } else if targets.contains(&"bin") {
        BridgeModel::Bin(find_bindings(&deps, &packages))
    } else {
        bail!("Couldn't detect the binding type; Please specify them with --bindings/-b")
    };

    if !(bridge.is_bindings("pyo3") || bridge.is_bindings("pyo3-ffi")) {
        eprintln!("🔗 Found {bridge} bindings");
    }

    for &lib in PYO3_BINDING_CRATES.iter() {
        if !bridge.is_bin() && bridge.is_bindings(lib) {
            let pyo3_node = deps[lib];
            if !pyo3_node.features.contains(&"extension-module".to_string()) {
                let version = cargo_metadata[&pyo3_node.id].version.to_string();
                eprintln!(
                    "⚠️  Warning: You're building a library without activating {lib}'s \
                     `extension-module` feature. \
                     See https://pyo3.rs/v{version}/building_and_distribution.html#linking"
                );
            }

            return if let Some((major, minor)) = has_abi3(cargo_metadata)? {
                eprintln!("🔗 Found {lib} bindings with abi3 support for Python ≥ {major}.{minor}");
                Ok(BridgeModel::BindingsAbi3(major, minor))
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
    min_python_minor: Option<usize>,
) -> Result<Vec<PythonInterpreter>> {
    let mut interpreters = Vec::new();
    if !interpreter.is_empty() {
        let mut missing = Vec::new();
        for interp in interpreter {
            match PythonInterpreter::check_executable(interp.clone(), target, bridge) {
                Ok(Some(interp)) => interpreters.push(interp),
                _ => missing.push(interp.clone()),
            }
        }
        if !missing.is_empty() {
            let sysconfig_interps =
                find_interpreter_in_sysconfig(&missing, target, min_python_minor)?;
            interpreters.extend(sysconfig_interps);
        }
    } else {
        interpreters = PythonInterpreter::find_all(target, bridge, min_python_minor)
            .context("Finding python interpreters failed")?;
    };

    if interpreters.is_empty() {
        if let Some(minor) = min_python_minor {
            bail!("Couldn't find any python interpreters with version >= 3.{}. Please specify at least one with -i", minor);
        } else {
            bail!("Couldn't find any python interpreters. Please specify at least one with -i");
        }
    }
    Ok(interpreters)
}

/// Find python interpreters in the host machine
fn find_interpreter_in_host(
    bridge: &BridgeModel,
    interpreter: &[PathBuf],
    target: &Target,
    min_python_minor: Option<usize>,
) -> Result<Vec<PythonInterpreter>> {
    let interpreters = if !interpreter.is_empty() {
        PythonInterpreter::check_executables(interpreter, target, bridge)
            .context("The given list of python interpreters is invalid")?
    } else {
        PythonInterpreter::find_all(target, bridge, min_python_minor)
            .context("Finding python interpreters failed")?
    };

    if interpreters.is_empty() {
        if let Some(minor) = min_python_minor {
            bail!("Couldn't find any python interpreters with version >= 3.{}. Please specify at least one with -i", minor);
        } else {
            bail!("Couldn't find any python interpreters. Please specify at least one with -i");
        }
    }
    Ok(interpreters)
}

/// Find python interpreters in the bundled sysconfig
fn find_interpreter_in_sysconfig(
    interpreter: &[PathBuf],
    target: &Target,
    min_python_minor: Option<usize>,
) -> Result<Vec<PythonInterpreter>> {
    if interpreter.is_empty() {
        return Ok(PythonInterpreter::find_by_target(target, min_python_minor));
    }
    let mut interpreters = Vec::new();
    for interp in interpreter {
        let python = interp.display().to_string();
        let (python_impl, python_ver) = if let Some(ver) = python.strip_prefix("pypy") {
            (InterpreterKind::PyPy, ver.strip_prefix('-').unwrap_or(ver))
        } else if let Some(ver) = python.strip_prefix("python") {
            (
                InterpreterKind::CPython,
                ver.strip_prefix('-').unwrap_or(ver),
            )
        } else if python
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            // Eg: -i 3.9 without interpreter kind, assume it's CPython
            (InterpreterKind::CPython, &*python)
        } else {
            bail!("Unsupported Python interpreter: {}", python);
        };
        let (ver_major, ver_minor) = python_ver
            .split_once('.')
            .context("Invalid python interpreter version")?;
        let ver_major = ver_major.parse::<usize>().with_context(|| {
            format!("Invalid python interpreter major version '{ver_major}', expect a digit")
        })?;
        let ver_minor = ver_minor.parse::<usize>().with_context(|| {
            format!("Invalid python interpreter minor version '{ver_minor}', expect a digit")
        })?;
        let sysconfig = InterpreterConfig::lookup(
            target.target_os(),
            target.target_arch(),
            python_impl,
            (ver_major, ver_minor),
        )
        .with_context(|| {
            format!("Failed to find a {python_impl} {ver_major}.{ver_minor} interpreter")
        })?;
        debug!(
            "Found {} {}.{} in bundled sysconfig",
            sysconfig.interpreter_kind, sysconfig.major, sysconfig.minor,
        );
        interpreters.push(PythonInterpreter::from_config(sysconfig.clone()));
    }
    Ok(interpreters)
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
            self.manifest_path = tool_maturin.manifest_path.clone();
            args_from_pyproject.push("manifest-path");
        }

        if self.profile.is_none() && tool_maturin.profile.is_some() {
            self.profile = tool_maturin.profile.clone();
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
            .manifest_path(&Path::new("test-crates/pyo3-mixed").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_mixed, None),
            Ok(BridgeModel::Bindings(..))
        ));
        assert!(matches!(
            find_bridge(&pyo3_mixed, Some("pyo3")),
            Ok(BridgeModel::Bindings(..))
        ));

        assert!(find_bridge(&pyo3_mixed, Some("rust-cpython")).is_err());
    }

    #[test]
    fn test_find_bridge_pyo3_abi3() {
        let pyo3_pure = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/pyo3-pure").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_pure, None),
            Ok(BridgeModel::BindingsAbi3(3, 7))
        ));
        assert!(matches!(
            find_bridge(&pyo3_pure, Some("pyo3")),
            Ok(BridgeModel::BindingsAbi3(3, 7))
        ));
        assert!(find_bridge(&pyo3_pure, Some("rust-cpython")).is_err());
    }

    #[test]
    fn test_find_bridge_pyo3_feature() {
        let pyo3_pure = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/pyo3-feature").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(find_bridge(&pyo3_pure, None).is_err());

        let pyo3_pure = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/pyo3-feature").join("Cargo.toml"))
            .other_options(vec!["--features=pyo3".to_string()])
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_pure, None).unwrap(),
            BridgeModel::Bindings(..)
        ));
    }

    #[test]
    fn test_find_bridge_cffi() {
        let cffi_pure = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/cffi-pure").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert_eq!(
            find_bridge(&cffi_pure, Some("cffi")).unwrap(),
            BridgeModel::Cffi
        );
        assert_eq!(find_bridge(&cffi_pure, None).unwrap(), BridgeModel::Cffi);

        assert!(find_bridge(&cffi_pure, Some("rust-cpython")).is_err());
        assert!(find_bridge(&cffi_pure, Some("pyo3")).is_err());
    }

    #[test]
    fn test_find_bridge_bin() {
        let hello_world = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/hello-world").join("Cargo.toml"))
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

        assert!(find_bridge(&hello_world, Some("rust-cpython")).is_err());
        assert!(find_bridge(&hello_world, Some("pyo3")).is_err());

        let pyo3_bin = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/pyo3-bin").join("Cargo.toml"))
            .exec()
            .unwrap();
        assert!(matches!(
            find_bridge(&pyo3_bin, Some("bin")).unwrap(),
            BridgeModel::Bin(Some((..)))
        ));
        assert!(matches!(
            find_bridge(&pyo3_bin, None).unwrap(),
            BridgeModel::Bin(Some(..))
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

    #[test]
    fn test_get_min_python_minor() {
        use crate::CargoToml;

        // Nothing specified
        let manifest_path = "test-crates/pyo3-pure/Cargo.toml";
        let cargo_toml = CargoToml::from_path(manifest_path).unwrap();
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_path)
            .exec()
            .unwrap();
        let metadata21 =
            Metadata21::from_cargo_toml(&cargo_toml, "test-crates/pyo3-pure", &cargo_metadata)
                .unwrap();
        assert_eq!(get_min_python_minor(&metadata21), None);
    }
}
