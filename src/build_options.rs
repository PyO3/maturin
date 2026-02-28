use crate::auditwheel::{AuditWheelMode, PlatformTag};
use crate::build_context::BuildContextBuilder;
use crate::compression::CompressionOptions;
use crate::pyproject_toml::{FeatureSpec, ToolMaturin};
use anyhow::Result;
use cargo_options::heading;
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::str::FromStr;
use tracing::instrument;

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
    use crate::bridge::{Abi3Version, PyO3, PyO3Crate, find_bridge};
    use crate::python_interpreter::InterpreterResolver;
    use crate::{BridgeModel, Target};
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
