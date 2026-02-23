use crate::auditwheel::{AuditWheelMode, PlatformTag};
use crate::bridge::{find_bridge, is_generating_import_lib};
use crate::compile::{CompileTarget, LIB_CRATE_TYPES};
use crate::compression::CompressionOptions;
use crate::project_layout::ProjectResolver;
use crate::pyproject_toml::{FeatureSpec, ToolMaturin};
use crate::python_interpreter::InterpreterResolver;
use crate::target::{
    detect_arch_from_python, detect_target_from_cross_python, is_arch_supported_by_pypi,
};
use crate::{BridgeModel, BuildContext, Target};
use anyhow::{Result, bail};
use cargo_metadata::CrateType;
use cargo_metadata::Metadata;
use cargo_options::heading;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{debug, instrument};

/// A Rust target triple or a virtual target triple.
#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
pub enum TargetTriple {
    /// The virtual `universal2-apple-darwin` target triple, build a fat binary of
    /// `aarch64-apple-darwin` and `x86_64-apple-darwin`.
    Universal2,
    /// Any target triple supported by Rust.
    ///
    /// It's not guaranteed that the value exists, it's passed verbatim to Cargo.
    Regular(String),
}

impl FromStr for TargetTriple {
    // TODO: Use the never type once stabilized
    type Err = String;

    fn from_str(triple: &str) -> std::result::Result<Self, Self::Err> {
        match triple {
            "universal2-apple-darwin" => Ok(TargetTriple::Universal2),
            triple => Ok(TargetTriple::Regular(triple.to_string())),
        }
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
    pub target: Option<TargetTriple>,

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
    /// Control the platform tag and PyPI compatibility.
    ///
    /// This options offers both fine-grained control over the linux libc tag and a more automatic
    /// PyPI-compatibility option.
    ///
    /// The `pypi` option applies on all platforms and ensure that only tags that can be uploaded to
    /// PyPI are used. The linux-specific options are `manylinux` tags (for example
    /// `manylinux2014`/`manylinux_2_24`) or `musllinux` tags (for example `musllinux_1_2`),
    /// and `linux` for the native linux tag. They are ignored on non-linux platforms.
    ///
    /// Note that `manylinux1` and `manylinux2010` are unsupported by the rust compiler.
    /// Wheels with the native `linux` tag will be rejected by pypi,
    /// unless they are separately validated by `auditwheel`.
    ///
    /// The default is the lowest compatible `manylinux` tag, or plain `linux` if nothing matched.
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

    /// Include debug info files (.pdb on Windows, .dSYM on macOS, .dwp on Linux)
    /// in the wheel. When enabled, maturin automatically configures
    /// split-debuginfo=packed so that separate debug info files are produced.
    #[arg(long)]
    pub include_debuginfo: bool,

    /// Cargo build options
    #[command(flatten)]
    pub cargo: CargoOptions,

    /// Additional SBOM files to include in the `.dist-info/sboms` directory.
    /// Can be specified multiple times.
    #[arg(long = "sbom-include", num_args = 1.., action = clap::ArgAction::Append)]
    pub sbom_include: Vec<PathBuf>,

    /// Wheel compression options
    #[command(flatten)]
    pub compression: CompressionOptions,
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
    /// Tries to fill the missing metadata for a BuildContext by querying cargo and python
    #[instrument(skip_all)]
    pub fn into_build_context(self) -> BuildContextBuilder {
        BuildContextBuilder::new(self)
    }
}

#[derive(Debug)]
pub struct BuildContextBuilder {
    build_options: BuildOptions,
    strip: Option<bool>,
    editable: bool,
    sdist_only: bool,
    pyproject_toml_path: Option<PathBuf>,
}

impl BuildContextBuilder {
    fn new(build_options: BuildOptions) -> Self {
        Self {
            build_options,
            strip: None,
            editable: false,
            sdist_only: false,
            pyproject_toml_path: None,
        }
    }

    pub fn strip(mut self, strip: Option<bool>) -> Self {
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

    pub fn pyproject_toml_path(mut self, path: Option<PathBuf>) -> Self {
        self.pyproject_toml_path = path;
        self
    }

    pub fn build(self) -> Result<BuildContext> {
        let Self {
            build_options,
            strip,
            editable,
            sdist_only,
            pyproject_toml_path: explicit_pyproject_path,
        } = self;
        build_options.compression.validate();
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
            editable,
            explicit_pyproject_path,
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
            pyproject,
        )?;
        debug!("Resolved bridge model: {:?}", bridge);

        if !bridge.is_bin() && project_layout.extension_name.contains('-') {
            bail!(
                "The module name must not contain a minus `-` \
                 (Make sure you have set an appropriate [lib] name or \
                 [tool.maturin] module-name in your pyproject.toml)"
            );
        }

        let (target, universal2) = resolve_target(
            build_options.target.clone(),
            build_options.interpreter.first(),
        )?;

        let wheel_dir = match build_options.out {
            Some(ref dir) => dir.clone(),
            None => PathBuf::from(&cargo_metadata.target_directory).join("wheels"),
        };

        let generate_import_lib = is_generating_import_lib(&cargo_metadata)?;
        let (interpreter, host_python) =
            if sdist_only && env::var_os("MATURIN_TEST_PYTHON").is_none() {
                // We don't need a python interpreter to build sdist only
                (Vec::new(), None)
            } else {
                let mut user_interpreters = build_options.interpreter.clone();

                // In test mode, allow MATURIN_TEST_PYTHON to override the default
                if cfg!(test)
                    && user_interpreters.is_empty()
                    && !build_options.find_interpreter
                    && let Some(python) = env::var_os("MATURIN_TEST_PYTHON")
                {
                    user_interpreters = vec![python.into()];
                }

                let resolver = InterpreterResolver::new(
                    &target,
                    &bridge,
                    metadata24.requires_python.as_ref(),
                    &user_interpreters,
                    build_options.find_interpreter,
                    generate_import_lib,
                );
                resolver.resolve()?
            };

        // Set PYO3_PYTHON for cross-compilation so pyo3's build script
        // can find the host interpreter.
        if let Some(ref host_python) = host_python {
            unsafe {
                env::set_var("PYO3_PYTHON", host_python);
                env::set_var("PYTHON_SYS_EXECUTABLE", host_python);
            }
        }

        if cargo_options.args.is_empty() {
            // if not supplied on command line, try pyproject.toml
            let tool_maturin = pyproject.and_then(|p| p.maturin());
            if let Some(args) = tool_maturin.and_then(|x| x.rustc_args.as_ref()) {
                cargo_options.args.extend(args.iter().cloned());
                pyproject_toml_maturin_options.push("rustc-args");
            }
        }

        let strip = strip.unwrap_or_else(|| pyproject.map(|x| x.strip()).unwrap_or_default());
        if strip && build_options.include_debuginfo {
            bail!("--include-debuginfo cannot be used with --strip");
        }
        let include_debuginfo = build_options.include_debuginfo;
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

        // Check if PyPI validation is needed before we move platform_tag
        let pypi_validation = build_options
            .platform_tag
            .iter()
            .any(|platform_tag| platform_tag == &PlatformTag::Pypi);

        let sbom = {
            let mut config = pyproject
                .and_then(|x| x.maturin())
                .and_then(|x| x.sbom.clone())
                .unwrap_or_default();
            if !build_options.sbom_include.is_empty() {
                let includes = config.include.get_or_insert_with(Vec::new);
                includes.extend(build_options.sbom_include.iter().cloned());
                includes.dedup();
            }
            Some(config)
        };

        let platform_tags = resolve_platform_tags(
            build_options.platform_tag,
            &target,
            &bridge,
            pyproject,
            &mut pyproject_toml_maturin_options,
            #[cfg(feature = "zig")]
            build_options.zig,
        )?;

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
            bail!(
                "No Cargo targets to build, please check your bindings configuration in pyproject.toml."
            );
        }

        let crate_name = cargo_toml.package.name;
        let include_import_lib = pyproject
            .map(|p| p.include_import_lib())
            .unwrap_or_default();
        // Extract conditional features from pyproject.toml if CLI features
        // didn't override (i.e. pyproject features were actually used)
        let conditional_features = if pyproject_toml_maturin_options.contains(&"features") {
            pyproject_toml
                .as_ref()
                .and_then(|p| p.maturin())
                .and_then(|m| m.features.clone())
                .map(|specs| FeatureSpec::split(specs).1)
                .unwrap_or_default()
        } else {
            Vec::new()
        };

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
            compression: build_options.compression,
            pypi_validation,
            sbom,
            include_import_lib,
            include_debuginfo,
            conditional_features,
        })
    }
}

/// Resolve the build target and universal2 flag from the user-specified
/// target triple (or `ARCHFLAGS`) and the first interpreter (if any).
fn resolve_target(
    target_triple: Option<TargetTriple>,
    first_interpreter: Option<&PathBuf>,
) -> Result<(Target, bool)> {
    let mut target_triple = target_triple;
    let mut universal2 = target_triple == Some(TargetTriple::Universal2);

    // Also try to determine universal2 from ARCHFLAGS environment variable
    if target_triple.is_none()
        && let Ok(arch_flags) = env::var("ARCHFLAGS")
    {
        let arches: HashSet<&str> = arch_flags
            .split("-arch")
            .filter_map(|x| {
                let x = x.trim();
                if x.is_empty() { None } else { Some(x) }
            })
            .collect();
        match (arches.contains("x86_64"), arches.contains("arm64")) {
            (true, true) => universal2 = true,
            (true, false) => {
                target_triple = Some(TargetTriple::Regular("x86_64-apple-darwin".to_string()))
            }
            (false, true) => {
                target_triple = Some(TargetTriple::Regular("aarch64-apple-darwin".to_string()))
            }
            (false, false) => {}
        }
    };
    if universal2 {
        target_triple = Some(TargetTriple::Regular("aarch64-apple-darwin".to_string()));
    }

    let mut target = Target::from_target_triple(target_triple.as_ref())?;
    if !target.user_specified && !universal2 {
        if let Some(interpreter) = first_interpreter {
            if let Some(detected_target) = detect_target_from_cross_python(interpreter) {
                target = Target::from_target_triple(Some(&detected_target))?;
            } else if let Some(detected_target) = detect_arch_from_python(interpreter, &target) {
                target = Target::from_target_triple(Some(&detected_target))?;
            }
        } else if let Some(detected_target) = detect_target_from_cross_python(&target.get_python())
        {
            target = Target::from_target_triple(Some(&detected_target))?;
        }
    }

    Ok((target, universal2))
}

/// Resolve platform tags from CLI flags, pyproject.toml, and target properties.
fn resolve_platform_tags(
    user_tags: Vec<PlatformTag>,
    target: &Target,
    bridge: &BridgeModel,
    pyproject: Option<&crate::pyproject_toml::PyProjectToml>,
    pyproject_options: &mut Vec<&str>,
    #[cfg(feature = "zig")] use_zig: bool,
) -> Result<Vec<PlatformTag>> {
    let platform_tags = if user_tags.is_empty() {
        #[cfg(feature = "zig")]
        let zig = use_zig;
        #[cfg(not(feature = "zig"))]
        let zig = false;
        let compatibility = pyproject
            .and_then(|x| {
                if x.compatibility().is_some() {
                    pyproject_options.push("compatibility");
                }
                x.compatibility()
            })
            .or(if zig {
                if target.is_musl_libc() {
                    Some(PlatformTag::Musllinux { major: 1, minor: 2 })
                } else {
                    Some(target.get_minimum_manylinux_tag())
                }
            } else if target.is_musl_libc() && !bridge.is_bin() {
                Some(PlatformTag::Musllinux { major: 1, minor: 2 })
            } else {
                None
            });
        if let Some(platform_tag) = compatibility {
            vec![platform_tag]
        } else {
            Vec::new()
        }
    } else if let [PlatformTag::Pypi] = &user_tags[..] {
        if !is_arch_supported_by_pypi(target) {
            bail!("Rust target {target} is not supported by PyPI");
        }
        Vec::new()
    } else {
        if user_tags.iter().any(|tag| tag.is_pypi()) && !is_arch_supported_by_pypi(target) {
            bail!("Rust target {target} is not supported by PyPI");
        }
        user_tags
            .into_iter()
            .filter(|platform_tag| platform_tag != &PlatformTag::Pypi)
            .collect()
    };

    for platform_tag in &platform_tags {
        if !platform_tag.is_supported() {
            eprintln!("⚠️  Warning: {platform_tag} is unsupported by the Rust compiler.");
        } else if platform_tag.is_musllinux() && !target.is_musl_libc() {
            eprintln!("⚠️  Warning: {target} is not compatible with {platform_tag}.");
        }
    }

    Ok(platform_tags)
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
    let resolved_features: Vec<String> = cargo_metadata
        .resolve
        .as_ref()
        .and_then(|resolve| resolve.nodes.iter().find(|&node| node.id == root_pkg.id))
        .map(|node| node.features.iter().map(|f| f.to_string()).collect())
        .unwrap_or_default();
    let mut targets: Vec<_> = root_pkg
        .targets
        .iter()
        .filter(|&target| match bridge {
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

/// We need to pass the global flags to cargo metadata
/// (https://github.com/PyO3/maturin/issues/211 and https://github.com/PyO3/maturin/issues/472),
/// but we can't pass all the extra args, as e.g. `--target` isn't supported, so this tries to
/// extract the arguments for cargo metadata or convert them to suitable forms
/// instead.
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
    // Target makes a difference during dependency resolving: cargo-fetch don't
    // bother with unnecessary dependencies on the given triple, but without
    // any target information supplied to cargo-metadata, it may consider them
    // necessary, which could fail the build with --offline supplied at the
    // same time.
    //
    // cargo-metadata does support --filter-platform to narrow the dependency
    // graph since 1.40, thus let's convert --target to it to make sure
    // cargo-metadata resolves the dependency as expected.
    if let Some(target) = &cargo_options.target {
        match target {
            TargetTriple::Universal2 => {
                cargo_metadata_extra_args.extend([
                    "--filter-platform".to_string(),
                    "aarch64-apple-darwin".to_string(),
                    "--filter-platform".to_string(),
                    "x86_64-apple-darwin".to_string(),
                ]);
            }
            TargetTriple::Regular(target) => {
                cargo_metadata_extra_args.push("--filter-platform".to_string());
                cargo_metadata_extra_args.push(target.clone());
            }
        }
    }
    for opt in &cargo_options.unstable_flags {
        cargo_metadata_extra_args.push("-Z".to_string());
        cargo_metadata_extra_args.push(opt.clone());
    }
    Ok(cargo_metadata_extra_args)
}

impl CargoOptions {
    /// Convert the Cargo options into a Cargo invocation.
    pub fn into_rustc_options(self, target_triple: Option<String>) -> cargo_options::Rustc {
        cargo_options::Rustc {
            common: cargo_options::CommonOptions {
                quiet: self.quiet,
                jobs: self.jobs,
                profile: self.profile,
                features: self.features,
                all_features: self.all_features,
                no_default_features: self.no_default_features,
                target: if let Some(target) = target_triple {
                    vec![target]
                } else {
                    Vec::new()
                },
                target_dir: self.target_dir,
                verbose: self.verbose,
                color: self.color,
                frozen: self.frozen,
                locked: self.locked,
                offline: self.offline,
                config: self.config,
                unstable_flags: self.unstable_flags,
                timings: self.timings,
                ..Default::default()
            },
            manifest_path: self.manifest_path,
            ignore_rust_version: self.ignore_rust_version,
            future_incompat_report: self.future_incompat_report,
            args: self.args,
            ..Default::default()
        }
    }
}

impl CargoOptions {
    /// Merge options from pyproject.toml
    pub fn merge_with_pyproject_toml(
        &mut self,
        tool_maturin: ToolMaturin,
        editable_install: bool,
    ) -> Vec<&'static str> {
        let mut args_from_pyproject = Vec::new();

        if self.manifest_path.is_none() && tool_maturin.manifest_path.is_some() {
            self.manifest_path.clone_from(&tool_maturin.manifest_path);
            args_from_pyproject.push("manifest-path");
        }

        if self.profile.is_none() {
            // For `maturin` v1 compatibility, `editable-profile` falls back to `profile` if unset.
            // TODO: on `maturin` v2, consider defaulting to "dev" profile for editable installs,
            // and potentially remove this fallback behavior.
            let (tool_profile, source_variable) =
                if editable_install && tool_maturin.editable_profile.is_some() {
                    (tool_maturin.editable_profile.as_ref(), "editable-profile")
                } else {
                    (tool_maturin.profile.as_ref(), "profile")
                };
            if let Some(tool_profile) = tool_profile {
                self.profile = Some(tool_profile.clone());
                args_from_pyproject.push(source_variable);
            }
        }

        if let Some(feature_specs) = tool_maturin.features
            && self.features.is_empty()
        {
            let (plain, _conditional) = FeatureSpec::split(feature_specs);
            self.features = plain;
            args_from_pyproject.push("features");
        }

        if let Some(all_features) = tool_maturin.all_features
            && !self.all_features
        {
            self.all_features = all_features;
            args_from_pyproject.push("all-features");
        }

        if let Some(no_default_features) = tool_maturin.no_default_features
            && !self.no_default_features
        {
            self.no_default_features = no_default_features;
            args_from_pyproject.push("no-default-features");
        }

        if let Some(frozen) = tool_maturin.frozen
            && !self.frozen
        {
            self.frozen = frozen;
            args_from_pyproject.push("frozen");
        }

        if let Some(locked) = tool_maturin.locked
            && !self.locked
        {
            self.locked = locked;
            args_from_pyproject.push("locked");
        }

        if let Some(config) = tool_maturin.config
            && self.config.is_empty()
        {
            self.config = config;
            args_from_pyproject.push("config");
        }

        if let Some(unstable_flags) = tool_maturin.unstable_flags
            && self.unstable_flags.is_empty()
        {
            self.unstable_flags = unstable_flags;
            args_from_pyproject.push("unstable-flags");
        }

        args_from_pyproject
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{Abi3Version, PyO3, PyO3Crate};
    use cargo_metadata::MetadataCommand;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    #[test]
    fn test_find_bridge_pyo3() {
        let pyo3_mixed = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/pyo3-mixed").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_mixed, None, None),
            Ok(BridgeModel::PyO3 { .. })
        ));
        assert!(matches!(
            find_bridge(&pyo3_mixed, Some("pyo3"), None),
            Ok(BridgeModel::PyO3 { .. })
        ));
    }

    #[test]
    fn test_find_bridge_pyo3_abi3() {
        use crate::bridge::{PyO3Metadata, PyO3VersionMetadata};

        let pyo3_pure = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/pyo3-pure").join("Cargo.toml"))
            .exec()
            .unwrap();

        let bridge = BridgeModel::PyO3(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version: semver::Version::new(0, 27, 1),
            abi3: Some(Abi3Version::Version(3, 7)),
            metadata: Some(PyO3Metadata {
                cpython: PyO3VersionMetadata {
                    min_minor: 7,
                    max_minor: 14,
                },
                pypy: PyO3VersionMetadata {
                    min_minor: 11,
                    max_minor: 11,
                },
            }),
        });
        assert_eq!(find_bridge(&pyo3_pure, None, None).unwrap(), bridge);
        assert_eq!(find_bridge(&pyo3_pure, Some("pyo3"), None).unwrap(), bridge);
    }

    #[test]
    fn test_find_bridge_pyo3_feature() {
        let pyo3_pure = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/pyo3-feature").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(find_bridge(&pyo3_pure, None, None).is_err());

        let pyo3_pure = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/pyo3-feature").join("Cargo.toml"))
            .other_options(vec!["--features=pyo3".to_string()])
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_pure, None, None).unwrap(),
            BridgeModel::PyO3 { .. }
        ));
    }

    #[test]
    fn test_find_bridge_cffi() {
        let cffi_pure = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/cffi-pure").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert_eq!(
            find_bridge(&cffi_pure, Some("cffi"), None).unwrap(),
            BridgeModel::Cffi
        );
        assert_eq!(
            find_bridge(&cffi_pure, None, None).unwrap(),
            BridgeModel::Cffi
        );

        assert!(find_bridge(&cffi_pure, Some("pyo3"), None).is_err());
    }

    #[test]
    fn test_find_bridge_bin() {
        let hello_world = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/hello-world").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert_eq!(
            find_bridge(&hello_world, Some("bin"), None).unwrap(),
            BridgeModel::Bin(None)
        );
        assert_eq!(
            find_bridge(&hello_world, None, None).unwrap(),
            BridgeModel::Bin(None)
        );

        assert!(find_bridge(&hello_world, Some("pyo3"), None).is_err());

        let pyo3_bin = MetadataCommand::new()
            .manifest_path(Path::new("test-crates/pyo3-bin").join("Cargo.toml"))
            .exec()
            .unwrap();
        assert!(matches!(
            find_bridge(&pyo3_bin, Some("bin"), None).unwrap(),
            BridgeModel::Bin(Some(_))
        ));
        assert!(matches!(
            find_bridge(&pyo3_bin, None, None).unwrap(),
            BridgeModel::Bin(Some(_))
        ));
    }

    #[test]
    fn test_old_extra_feature_args() {
        let cargo_extra_args = CargoOptions {
            no_default_features: true,
            features: vec!["a".to_string(), "c".to_string()],
            target: Some(TargetTriple::Regular(
                "x86_64-unknown-linux-musl".to_string(),
            )),
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
                "--filter-platform",
                "x86_64-unknown-linux-musl",
            ]
        );
    }

    #[test]
    fn test_extract_cargo_metadata_args() {
        let args = CargoOptions {
            locked: true,
            features: vec!["my-feature".to_string(), "other-feature".to_string()],
            target: Some(TargetTriple::Regular(
                "x86_64-unknown-linux-musl".to_string(),
            )),
            unstable_flags: vec!["unstable-options".to_string()],
            ..Default::default()
        };

        let expected = vec![
            "--locked",
            "--features",
            "my-feature",
            "--features",
            "other-feature",
            "--filter-platform",
            "x86_64-unknown-linux-musl",
            "-Z",
            "unstable-options",
        ];

        assert_eq!(extract_cargo_metadata_args(&args).unwrap(), expected);
    }

    #[test]
    fn test_find_single_python_interpreter_not_found() {
        let target = Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap();
        let bridge = BridgeModel::Cffi;
        let interpreter = vec![PathBuf::from("nonexistent-python-xyz")];

        let resolver = InterpreterResolver::new(&target, &bridge, None, &interpreter, false, false);
        let result = resolver.resolve();
        let err_msg = result.unwrap_err().to_string();
        assert_snapshot!(err_msg, @"Failed to find a python interpreter from `nonexistent-python-xyz`");
    }
}
