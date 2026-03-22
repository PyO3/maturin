use crate::auditwheel::{AuditWheelMode, PlatformTag};
use crate::build_context::BuildContextBuilder;
pub use crate::cargo_options::{CargoOptions, TargetTriple};
use crate::compression::CompressionOptions;
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use tracing::instrument;

/// Options for configuring the target Python environment and bindings.
///
/// These options define the 'Constraints' of the build.
#[derive(Debug, Default, Serialize, Deserialize, clap::Parser, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct PythonOptions {
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
}

/// Options for configuring platform tags and binary compatibility.
///
/// These options define the 'Constraints' of the build related to the OS and libc.
#[derive(Debug, Default, Serialize, Deserialize, clap::Parser, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct PlatformOptions {
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
}

/// Options for controlling final build artifacts and their metadata.
///
/// These options define the 'Output' part of the build.
#[derive(Debug, Default, Serialize, Deserialize, clap::Parser, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct OutputOptions {
    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    #[arg(short, long)]
    pub out: Option<PathBuf>,

    /// Include debug info files (.pdb on Windows, .dSYM on macOS, .dwp on Linux)
    /// in the wheel. When enabled, maturin automatically configures
    /// split-debuginfo=packed so that separate debug info files are produced.
    #[arg(long)]
    pub include_debuginfo: bool,

    /// Additional SBOM files to include in the `.dist-info/sboms` directory.
    /// Can be specified multiple times.
    #[arg(long = "sbom-include", num_args = 1.., action = clap::ArgAction::Append)]
    pub sbom_include: Vec<PathBuf>,
}

/// High level API for building wheels from a crate, also used for the CLI.
///
/// This struct is the primary entry point for build configuration and is
/// partitioned into modular sub-groups reflecting the build lifecycle.
#[derive(Debug, Default, Serialize, Deserialize, clap::Parser, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct BuildOptions {
    /// Python and bindings options
    #[command(flatten)]
    pub python: PythonOptions,

    /// Platform tag and auditwheel options
    #[command(flatten)]
    pub platform: PlatformOptions,

    /// Output artifact options
    #[command(flatten)]
    pub output: OutputOptions,

    /// Cargo build options
    #[command(flatten)]
    pub cargo: CargoOptions,

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{Abi3Version, PyO3, PyO3Crate, find_bridge};
    use crate::python_interpreter::InterpreterResolver;
    use crate::test_utils::test_crate_path;
    use crate::{BridgeModel, Target};
    use cargo_metadata::MetadataCommand;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_find_bridge_pyo3() {
        let pyo3_mixed = MetadataCommand::new()
            .manifest_path(test_crate_path("pyo3-mixed").join("Cargo.toml"))
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
            .manifest_path(test_crate_path("pyo3-pure").join("Cargo.toml"))
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
            .manifest_path(test_crate_path("pyo3-feature").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(find_bridge(&pyo3_pure, None, None).is_err());

        let pyo3_pure = MetadataCommand::new()
            .manifest_path(test_crate_path("pyo3-feature").join("Cargo.toml"))
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
            .manifest_path(test_crate_path("cffi-pure").join("Cargo.toml"))
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
            .manifest_path(test_crate_path("hello-world").join("Cargo.toml"))
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
            .manifest_path(test_crate_path("pyo3-bin").join("Cargo.toml"))
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
