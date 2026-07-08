use crate::auditwheel::{AuditWheelMode, CompatibilityTag};
use crate::build_context::BuildContextBuilder;
pub use crate::cargo_options::{CargoOptions, TargetTriple};
use crate::compression::CompressionOptions;
use serde::{Deserialize, Serialize};
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
    pub platform_tag: Vec<CompatibilityTag>,

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

    /// Auto generate Python type stubs by introspecting the binary. Requires PyO3 and its "experimental-inspect" feature
    #[arg(long)]
    pub generate_stubs: bool,
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
    use crate::bridge::{PyO3, PyO3Crate, StableAbi, StableAbiKind, StableAbiVersion, find_bridge};
    use crate::python_interpreter::InterpreterResolver;
    use crate::test_utils::test_crate_path;
    use crate::{BridgeModel, Target};
    use cargo_metadata::{CargoOpt, MetadataCommand};
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_find_bridge_pyo3() {
        let pyo3_mixed = MetadataCommand::new()
            .manifest_path(test_crate_path("pyo3-mixed").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_mixed, None),
            Ok(BridgeModel::PyO3 { .. })
        ));
        assert!(matches!(
            find_bridge(&pyo3_mixed, Some("pyo3")),
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
            version: semver::Version::new(0, 29, 0),
            stable_abi: Some(StableAbi::from_abi3_version(3, 9)),
            metadata: Some(PyO3Metadata {
                cpython: PyO3VersionMetadata {
                    min_minor: 8,
                    max_minor: 15,
                },
                pypy: PyO3VersionMetadata {
                    min_minor: 11,
                    max_minor: 11,
                },
            }),
        });
        assert_eq!(find_bridge(&pyo3_pure, None).unwrap(), bridge);
        assert_eq!(find_bridge(&pyo3_pure, Some("pyo3")).unwrap(), bridge);
    }

    #[test]
    fn test_find_bridge_pyo3_abi3t() {
        let pyo3_abi3t = MetadataCommand::new()
            .manifest_path(test_crate_path("pyo3-abi3t").join("Cargo.toml"))
            .exec()
            .unwrap();

        let bridge = find_bridge(&pyo3_abi3t, None).unwrap();
        let pyo3 = match &bridge {
            BridgeModel::PyO3(pyo3) => pyo3,
            other => panic!("expected PyO3 bridge, got {other:?}"),
        };
        assert_eq!(
            pyo3.stable_abi,
            Some(StableAbi::from_abi3t_version(3, 15)),
            "abi3t-py315 should produce a StableAbiKind::Abi3t with min version 3.15"
        );
        assert_eq!(pyo3.crate_name, PyO3Crate::PyO3);

        let bridge_explicit = find_bridge(&pyo3_abi3t, Some("pyo3")).unwrap();
        assert_eq!(bridge_explicit, bridge);
    }

    #[test]
    fn test_find_bridge_pyo3_abi3t_without_version() {
        let pyo3_abi3t_without_version = MetadataCommand::new()
            .manifest_path(test_crate_path("pyo3-abi3t-without-version").join("Cargo.toml"))
            .exec()
            .unwrap();

        let bridge = find_bridge(&pyo3_abi3t_without_version, None).unwrap();
        let pyo3 = match &bridge {
            BridgeModel::PyO3(pyo3) => pyo3,
            other => panic!("expected PyO3 bridge, got {other:?}"),
        };
        // The `abi3t` feature alone (no `abi3t-py3XY`) should map to CurrentPython.
        let stable_abi = pyo3.stable_abi.expect("expected abi3t stable_abi");
        assert!(
            matches!(stable_abi.kind, StableAbiKind::Abi3t),
            "expected StableAbiKind::Abi3t, got {:?}",
            stable_abi.kind
        );
        assert_eq!(stable_abi.version, StableAbiVersion::CurrentPython);
    }

    #[test]
    fn test_find_bridge_pyo3_combined_abi3_and_abi3t() {
        let cases = [
            (None, StableAbi::from_abi3_version(3, 8)),
            (
                Some("abi3-and-current-abi3t"),
                StableAbi::from_abi3_version(3, 8),
            ),
        ];

        for (feature, expected) in cases {
            let mut command = MetadataCommand::new();
            command.manifest_path(test_crate_path("pyo3-abi3-and-abi3t").join("Cargo.toml"));
            if let Some(feature) = feature {
                command.features(CargoOpt::NoDefaultFeatures);
                command.features(CargoOpt::SomeFeatures(vec![feature.to_string()]));
            }
            let metadata = command.exec().unwrap();

            let bridge = find_bridge(&metadata, None).unwrap();
            let pyo3 = match &bridge {
                BridgeModel::PyO3(pyo3) => pyo3,
                other => panic!("expected PyO3 bridge, got {other:?}"),
            };
            assert_eq!(
                pyo3.stable_abi,
                Some(expected),
                "unexpected stable ABI for feature {feature:?}",
            );
        }
    }

    #[test]
    fn test_upgrade_bridge_pyo3_combined_abi3_and_abi3t_selects_single_abi() {
        use crate::bridge::upgrade_bridge_stable_abi;
        use crate::python_interpreter::{InterpreterConfig, InterpreterKind};

        let metadata = MetadataCommand::new()
            .manifest_path(test_crate_path("pyo3-abi3-and-abi3t").join("Cargo.toml"))
            .exec()
            .unwrap();
        let bridge = find_bridge(&metadata, None).unwrap();
        let target = Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap();
        let cpython = |minor, abiflags| {
            crate::PythonInterpreter::from_config(
                InterpreterConfig::lookup_one(
                    &target,
                    InterpreterKind::CPython,
                    (3, minor),
                    abiflags,
                )
                .unwrap(),
            )
        };
        let cases = [
            (
                "only GIL-enabled 3.14",
                vec![cpython(14, "")],
                StableAbi::from_abi3_version(3, 8),
            ),
            (
                "only free-threaded 3.14",
                vec![cpython(14, "t")],
                StableAbi::from_abi3_version(3, 8),
            ),
            (
                "only free-threaded 3.15",
                vec![cpython(15, "t")],
                StableAbi::from_abi3t_version(3, 15),
            ),
            (
                "only GIL-enabled 3.15",
                vec![cpython(15, "")],
                StableAbi::from_abi3t_version(3, 15),
            ),
            (
                "GIL-enabled 3.14 and free-threaded 3.15",
                vec![cpython(14, ""), cpython(15, "t")],
                StableAbi::from_abi3t_version(3, 15),
            ),
        ];

        for (name, interpreters, expected) in cases {
            let bridge =
                upgrade_bridge_stable_abi(bridge.clone(), &metadata, None, &interpreters).unwrap();
            let stable_abi = bridge.pyo3().and_then(|pyo3| pyo3.stable_abi);
            assert_eq!(stable_abi, Some(expected), "{name}");
        }
    }

    /// Mirrors `test_find_bridge_pyo3_abi3t` for a crate that depends on `pyo3-ffi`
    /// directly (the FFI-only path, analogous to `pyo3-ffi-pure`). Verifies the
    /// crate_name is `PyO3Ffi` and version detection works the same as via `pyo3`.
    #[test]
    fn test_find_bridge_pyo3_ffi_abi3t_py315() {
        let pyo3_ffi_abi3t = MetadataCommand::new()
            .manifest_path(test_crate_path("pyo3-ffi-abi3t-py315").join("Cargo.toml"))
            .exec()
            .unwrap();

        let bridge = find_bridge(&pyo3_ffi_abi3t, None).unwrap();
        let pyo3 = match &bridge {
            BridgeModel::PyO3(pyo3) => pyo3,
            other => panic!("expected PyO3 bridge, got {other:?}"),
        };
        assert_eq!(
            pyo3.stable_abi,
            Some(StableAbi::from_abi3t_version(3, 15)),
            "abi3t-py315 on pyo3-ffi must produce a Version(3, 15) abi3t bridge"
        );
        assert_eq!(pyo3.crate_name, PyO3Crate::PyO3Ffi);
    }

    /// `is_abi3_for_interpreter` must distinguish between the abi3 and abi3t kinds:
    /// only abi3 kinds may emit the `cp{ver}-abi3-{platform}` linker name. abi3t
    /// builds need to fall through to `is_stable_abi_for_interpreter`, which is the
    /// kind-agnostic check used to decide whether to define `Py_LIMITED_API`.
    #[test]
    fn test_stable_abi_for_interpreter_distinguishes_kinds() {
        use crate::bridge::{PyO3Metadata, PyO3VersionMetadata};
        use crate::python_interpreter::{InterpreterConfig, InterpreterKind};
        use crate::{PythonInterpreter, Target};

        let target = Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap();

        // GIL-enabled CPython 3.15 - supports abi3 and abi3t
        let py315 = PythonInterpreter::from_config(
            InterpreterConfig::lookup_one(&target, InterpreterKind::CPython, (3, 15), "").unwrap(),
        );
        // Free-threaded CPython 3.15 - supports abi3t and does NOT support abi3
        let py315t = PythonInterpreter::from_config(
            InterpreterConfig::lookup_one(&target, InterpreterKind::CPython, (3, 15), "t").unwrap(),
        );
        // Free-threaded CPython 3.14 — does NOT support abi3 or abi3t
        let py314t = PythonInterpreter::from_config(
            InterpreterConfig::lookup_one(&target, InterpreterKind::CPython, (3, 14), "t").unwrap(),
        );
        // GIL-enabled CPython 3.14 - supports abi3 and does NOT support abi3t
        let py314 = PythonInterpreter::from_config(
            InterpreterConfig::lookup_one(&target, InterpreterKind::CPython, (3, 14), "").unwrap(),
        );

        let metadata = PyO3Metadata {
            cpython: PyO3VersionMetadata {
                min_minor: 7,
                max_minor: 15,
            },
            pypy: PyO3VersionMetadata {
                min_minor: 11,
                max_minor: 11,
            },
        };

        let abi3_bridge = BridgeModel::PyO3(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version: semver::Version::new(0, 28, 3),
            stable_abi: Some(StableAbi::from_abi3_version(3, 9)),
            metadata: Some(metadata.clone()),
        });
        let abi3t_bridge = BridgeModel::PyO3(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version: semver::Version::new(0, 28, 3),
            stable_abi: Some(StableAbi::from_abi3t_version(3, 15)),
            metadata: Some(metadata),
        });

        assert_eq!(
            abi3t_bridge.pyo3().unwrap().stable_abi.unwrap().kind,
            StableAbiKind::Abi3t
        );
        assert_eq!(
            abi3_bridge.pyo3().unwrap().stable_abi.unwrap().kind,
            StableAbiKind::Abi3
        );

        assert!(abi3_bridge.is_stable_abi_for_interpreter(&py315));
        assert!(abi3t_bridge.is_stable_abi_for_interpreter(&py315));
        assert!(abi3t_bridge.is_stable_abi_for_interpreter(&py315t));

        // abi3 isn't supported for free-threaded interpreters, should fail
        assert!(!abi3_bridge.is_stable_abi_for_interpreter(&py315t));

        // Free-threaded 3.14: no stable api support
        assert!(!abi3t_bridge.is_stable_abi_for_interpreter(&py314t));
        assert!(!abi3_bridge.is_stable_abi_for_interpreter(&py314t));

        // GIL-enabled 3.14 supports abi3 but not abi3t
        assert!(!abi3t_bridge.is_stable_abi_for_interpreter(&py314));
        assert!(abi3_bridge.is_stable_abi_for_interpreter(&py314));
    }

    #[test]
    fn test_find_bridge_pyo3_feature() {
        let pyo3_pure = MetadataCommand::new()
            .manifest_path(test_crate_path("pyo3-feature").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(find_bridge(&pyo3_pure, None).is_err());

        let pyo3_pure = MetadataCommand::new()
            .manifest_path(test_crate_path("pyo3-feature").join("Cargo.toml"))
            .other_options(vec!["--features=pyo3".to_string()])
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_pure, None).unwrap(),
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
            find_bridge(&cffi_pure, Some("cffi")).unwrap(),
            BridgeModel::Cffi
        );
        assert_eq!(find_bridge(&cffi_pure, None).unwrap(), BridgeModel::Cffi);

        assert!(find_bridge(&cffi_pure, Some("pyo3")).is_err());
    }

    #[test]
    fn test_find_bridge_bin() {
        let hello_world = MetadataCommand::new()
            .manifest_path(test_crate_path("hello-world").join("Cargo.toml"))
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
            .manifest_path(test_crate_path("pyo3-bin").join("Cargo.toml"))
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
    fn test_find_bridge_conditional_abi3_filtered_by_interpreter() {
        use crate::bridge::upgrade_bridge_stable_abi;
        use crate::python_interpreter::InterpreterConfig;

        // A pyproject.toml with pyo3/abi3-py311 gated on python-version >= 3.11
        let pyproject: crate::PyProjectToml = toml::from_str(
            r#"
            [build-system]
            requires = ["maturin"]
            build-backend = "maturin"

            [tool.maturin]
            features = [{ feature = "pyo3/abi3-py311", python-version = ">=3.11" }]
            "#,
        )
        .unwrap();

        // Use pyo3-mixed which has no abi3 in Cargo.toml, so abi3 inference
        // depends entirely on the conditional pyproject feature.
        let metadata = MetadataCommand::new()
            .manifest_path(test_crate_path("pyo3-mixed").join("Cargo.toml"))
            .exec()
            .unwrap();

        let target = Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap();

        // find_bridge alone never includes conditional features → no abi3
        let bridge = find_bridge(&metadata, None).unwrap();
        assert!(
            !bridge.has_stable_abi(),
            "find_bridge should not infer abi3 from conditional features"
        );

        // With a Python 3.10 interpreter, condition doesn't match → no abi3
        let py310 = crate::PythonInterpreter::from_config(
            InterpreterConfig::lookup_one(
                &target,
                crate::python_interpreter::InterpreterKind::CPython,
                (3, 10),
                "",
            )
            .unwrap(),
        );
        let bridge = upgrade_bridge_stable_abi(
            bridge,
            &metadata,
            Some(&pyproject),
            std::slice::from_ref(&py310),
        )
        .unwrap();
        assert!(
            !bridge.has_stable_abi(),
            "should not infer abi3 for Python 3.10"
        );

        // With a Python 3.11 interpreter, condition matches → abi3
        let py311 = crate::PythonInterpreter::from_config(
            InterpreterConfig::lookup_one(
                &target,
                crate::python_interpreter::InterpreterKind::CPython,
                (3, 11),
                "",
            )
            .unwrap(),
        );
        let base_bridge = find_bridge(&metadata, None).unwrap();
        let bridge = upgrade_bridge_stable_abi(
            base_bridge,
            &metadata,
            Some(&pyproject),
            std::slice::from_ref(&py311),
        )
        .unwrap();
        assert!(bridge.has_stable_abi(), "should infer abi3 for Python 3.11");

        // With mixed interpreters [3.10, 3.11], abi3 IS inferred because
        // at least one interpreter (3.11) matches the condition. This is safe
        // because build_stable_abi_wheels splits interpreters by min_version:
        // 3.10 gets a version-specific wheel, 3.11+ gets the abi3 wheel.
        let base_bridge = find_bridge(&metadata, None).unwrap();
        let bridge =
            upgrade_bridge_stable_abi(base_bridge, &metadata, Some(&pyproject), &[py310, py311])
                .unwrap();
        assert!(
            bridge.has_stable_abi(),
            "should infer abi3 for mixed [3.10, 3.11] (build_stable_abi_wheels handles the split)"
        );
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
