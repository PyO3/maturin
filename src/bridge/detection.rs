//! Bridge model detection from cargo metadata.
//!
//! This module inspects the dependency graph and feature flags of a crate
//! to determine which binding model (pyo3, cffi, uniffi, bin) to use,
//! whether abi3 is enabled, and whether `generate-import-lib` is active.

use super::{
    BridgeModel, PyO3, PyO3Crate, PyO3MetadataRaw, StableAbi, StableAbiKind, StableAbiVersion,
};
use crate::PyProjectToml;
use crate::pyproject_toml::{FeatureConditionEnv, FeatureSpec};
use anyhow::{Context, Result, bail};
use cargo_metadata::{CrateType, Metadata, Node, PackageId, TargetKind};
use std::collections::{HashMap, HashSet};

// pyo3-ffi is ordered first because it is newer and more restrictive.
const PYO3_BINDING_CRATES: [PyO3Crate; 2] = [PyO3Crate::PyO3Ffi, PyO3Crate::PyO3];

/// Tries to determine the [`BridgeModel`] for the target crate.
///
/// Inspects cargo metadata to detect pyo3/cffi/uniffi bindings, abi3 support,
/// and extension-module feature usage. If `bridge` is `Some`, the binding type
/// is forced; otherwise it's auto-detected from dependencies and target types.
///
/// Conditional pyo3/pyo3-ffi features from pyproject.toml are excluded from
/// abi3 inference here. Use [`upgrade_bridge_abi3`] after interpreter resolution
/// to evaluate them.
pub fn find_bridge(cargo_metadata: &Metadata, bridge: Option<&str>) -> Result<BridgeModel> {
    let no_extra_features = HashMap::new();
    let deps = current_crate_dependencies(cargo_metadata)?;
    let packages: HashMap<&str, &cargo_metadata::Package> = cargo_metadata
        .packages
        .iter()
        .filter_map(|pkg| {
            let name = pkg.name.as_ref();
            if name == "pyo3" || name == "pyo3-ffi" || name == "uniffi" {
                Some((name, pkg))
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
            let bindings = find_pyo3_bindings(&deps, &packages)?;
            BridgeModel::Bin(bindings)
        } else {
            let bindings = find_pyo3_bindings(&deps, &packages)?.context("unknown binding type")?;
            BridgeModel::PyO3(bindings)
        }
    } else {
        match find_pyo3_bindings(&deps, &packages)? {
            Some(bindings) => {
                if !targets.contains(&CrateType::CDyLib) && targets.contains(&CrateType::Bin) {
                    BridgeModel::Bin(Some(bindings))
                } else {
                    BridgeModel::PyO3(bindings)
                }
            }
            _ => {
                if deps.contains_key("uniffi") {
                    BridgeModel::UniFfi
                } else if targets.contains(&CrateType::CDyLib) {
                    BridgeModel::Cffi
                } else if targets.contains(&CrateType::Bin) {
                    BridgeModel::Bin(find_pyo3_bindings(&deps, &packages)?)
                } else {
                    bail!(
                        "Couldn't detect the binding type; Please specify them with --bindings/-b"
                    )
                }
            }
        }
    };

    if !bridge.is_pyo3() {
        return Ok(bridge);
    }

    for &lib in PYO3_BINDING_CRATES.iter() {
        if !bridge.is_bin() && bridge.is_pyo3_crate(lib) {
            let lib_name = lib.as_str();
            let pyo3_node = deps[lib_name];
            if !pyo3_node
                .features
                .iter()
                .map(AsRef::as_ref)
                .any(|f| f == "extension-module")
            {
                let version = &cargo_metadata[&pyo3_node.id].version;
                if (version.major, version.minor) < (0, 26) {
                    // pyo3 0.26+ will use the `PYO3_BUILD_EXTENSION_MODULE` env var instead
                    eprintln!(
                        "⚠️  Warning: You're building a library without activating {lib}'s \
                        `extension-module` feature. \
                        See https://pyo3.rs/v{version}/building-and-distribution.html#the-extension-module-feature"
                    );
                }
            }

            return if let Some(stable_abi) = has_stable_abi(&deps, &no_extra_features)? {
                let pyo3 = bridge.pyo3().expect("should be pyo3 bindings");
                let bindings = PyO3 {
                    crate_name: lib,
                    version: pyo3.version.clone(),
                    stable_abi: Some(stable_abi),
                    metadata: pyo3.metadata.clone(),
                };
                Ok(BridgeModel::PyO3(bindings))
            } else {
                Ok(bridge)
            };
        }
    }

    Ok(bridge)
}

/// Upgrade a bridge model to abi3 if conditional pyo3/pyo3-ffi features
/// from pyproject.toml match at least one of the given interpreters.
///
/// This is the second phase of bridge detection: [`find_bridge`] excludes
/// conditional features, then after interpreter resolution this function
/// re-checks whether any conditional abi3 feature applies.
pub fn upgrade_bridge_abi3(
    bridge: BridgeModel,
    cargo_metadata: &Metadata,
    pyproject: Option<&PyProjectToml>,
    interpreters: &[crate::PythonInterpreter],
) -> Result<BridgeModel> {
    // Only relevant for pyo3 bridges without abi3 already set
    let Some(pyo3) = bridge.pyo3() else {
        return Ok(bridge);
    };
    if pyo3.stable_abi.is_some() {
        return Ok(bridge);
    }

    let extra_pyo3_features = pyo3_features_from_conditional(pyproject, interpreters);
    if extra_pyo3_features.is_empty() {
        return Ok(bridge);
    }

    let deps = current_crate_dependencies(cargo_metadata)?;
    if let Some(stable_abi) = has_stable_abi(&deps, &extra_pyo3_features)? {
        let upgraded = PyO3 {
            stable_abi: Some(stable_abi),
            ..pyo3.clone()
        };
        return Ok(match bridge {
            BridgeModel::PyO3(_) => BridgeModel::PyO3(upgraded),
            BridgeModel::Bin(Some(_)) => BridgeModel::Bin(Some(upgraded)),
            _ => return Ok(bridge),
        });
    }

    Ok(bridge)
}

/// Check whether `generate-import-lib` feature is enabled in pyo3.
///
/// pyo3 0.16.4+ supports building abi3 wheels without a working Python interpreter
/// for Windows when `generate-import-lib` feature is enabled.
/// pyo3 0.29.0+ uses raw-dylib linking on Windows, so import library generation
/// is no longer needed and this effectively always returns true.
pub fn has_windows_import_lib_support(cargo_metadata: &Metadata) -> Result<bool> {
    let resolve = cargo_metadata
        .resolve
        .as_ref()
        .context("Expected cargo to return metadata with resolve")?;
    for &lib in PYO3_BINDING_CRATES.iter().rev() {
        let lib = lib.as_str();
        let pyo3_packages = resolve
            .nodes
            .iter()
            .filter(|package| cargo_metadata[&package.id].name.as_str() == lib)
            .collect::<Vec<_>>();
        match pyo3_packages.as_slice() {
            &[pyo3_crate] => {
                let pyo3_version = &cargo_metadata[&pyo3_crate.id].version;
                if pyo3_version >= &semver::Version::new(0, 29, 0) {
                    return Ok(true);
                }
                let generate_import_lib = pyo3_crate
                    .features
                    .iter()
                    .map(AsRef::as_ref)
                    .any(|x| x == "generate-import-lib" || x == "generate-abi3-import-lib");
                return Ok(generate_import_lib);
            }
            _ => continue,
        }
    }
    Ok(false)
}

fn has_stable_abi(
    deps: &HashMap<&str, &Node>,
    extra_features: &HashMap<&str, Vec<String>>,
) -> Result<Option<StableAbi>> {
    let abi3 = has_stable_abi_from_kind(deps, extra_features, StableAbiKind::Abi3)?;
    if abi3.is_some() {
        return Ok(abi3);
    }
    Ok(None)
}

/// pyo3 supports building stable abi wheels if the unstable-api feature is not selected
fn has_stable_abi_from_kind(
    deps: &HashMap<&str, &Node>,
    extra_features: &HashMap<&str, Vec<String>>,
    abi_kind: StableAbiKind,
) -> Result<Option<StableAbi>> {
    for &lib in PYO3_BINDING_CRATES.iter() {
        let lib = lib.as_str();
        if let Some(&pyo3_crate) = deps.get(lib) {
            let extra = extra_features.get(lib);
            // Find the minimal abi3 python version. If there is none, abi3 hasn't been selected
            // Find the minimal stable abi python version. If there is none, stable abi hasn't been selected
            // This parses abi3-py{major}{minor} and returns the minimal (major, minor) tuple
            let all_features: Vec<&str> = pyo3_crate
                .features
                .iter()
                .map(AsRef::as_ref)
                .chain(extra.into_iter().flatten().map(String::as_str))
                .collect();

            let abi_str = format!("{abi_kind}");
            let search_str = format!("{abi_kind}-py");
            let stable_abi_selected = all_features.contains(&abi_str.as_str());
            let offset = search_str.len();
            let filter_len = offset + 2;

            let min_stable_abi_version = all_features
                .iter()
                .filter(|&&x| x.starts_with(search_str.as_str()) && x.len() >= filter_len)
                .map(|x| {
                    Ok((
                        (x.as_bytes()[offset] as char).to_string().parse::<u8>()?,
                        x[offset + 1..].parse::<u8>()?,
                    ))
                })
                .collect::<Result<Vec<(u8, u8)>>>()
                .context(format!("Bogus {lib} cargo features"))?
                .into_iter()
                .min();
            match min_stable_abi_version {
                Some((major, minor)) => {
                    return Ok(Some(StableAbi {
                        kind: abi_kind,
                        version: StableAbiVersion::Version(major, minor),
                    }));
                }
                None if stable_abi_selected => {
                    return Ok(Some(StableAbi {
                        kind: abi_kind,
                        version: StableAbiVersion::CurrentPython,
                    }));
                }
                None => {}
            }
        }
    }
    Ok(None)
}

/// Tries to determine the bindings type from dependency
fn find_pyo3_bindings(
    deps: &HashMap<&str, &Node>,
    packages: &HashMap<&str, &cargo_metadata::Package>,
) -> anyhow::Result<Option<PyO3>> {
    if deps.get("pyo3").is_some() {
        let pyo3_metadata = match packages.get("pyo3-ffi") {
            Some(pyo3_ffi) => pyo3_ffi.metadata.clone(),
            None => {
                // Old versions of pyo3 does not depend on pyo3-ffi,
                // thus does not have the metadata
                serde_json::Value::Null
            }
        };
        let metadata = match serde_json::from_value::<Option<PyO3MetadataRaw>>(pyo3_metadata) {
            Ok(Some(metadata)) => Some(metadata.try_into()?),
            Ok(None) | Err(_) => None,
        };
        let version = packages["pyo3"].version.clone();
        Ok(Some(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version,
            stable_abi: None,
            metadata,
        }))
    } else if deps.get("pyo3-ffi").is_some() {
        let package = &packages["pyo3-ffi"];
        let version = package.version.clone();
        let metadata =
            match serde_json::from_value::<Option<PyO3MetadataRaw>>(package.metadata.clone()) {
                Ok(Some(metadata)) => Some(metadata.try_into()?),
                Ok(None) | Err(_) => None,
            };
        Ok(Some(PyO3 {
            crate_name: PyO3Crate::PyO3Ffi,
            version,
            stable_abi: None,
            metadata,
        }))
    } else {
        Ok(None)
    }
}

/// Return a map with all (transitive) dependencies of the *current* crate.
///
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

/// Extract pyo3/pyo3-ffi feature names from conditional features in pyproject.toml.
///
/// For a conditional feature like `pyo3/abi3-py311`, this extracts `abi3-py311`
/// for the corresponding binding crate.
fn pyo3_features_from_conditional(
    pyproject: Option<&PyProjectToml>,
    interpreters: &[crate::PythonInterpreter],
) -> HashMap<&'static str, Vec<String>> {
    let mut extra: HashMap<&'static str, Vec<String>> = HashMap::new();
    let features = match pyproject
        .and_then(|p| p.maturin())
        .and_then(|m| m.features.clone())
    {
        Some(f) => f,
        None => return extra,
    };
    let (_plain, conditional) = FeatureSpec::split(features);
    let crate_names: &[&str] = &["pyo3", "pyo3-ffi"];

    // Collect the union of conditional features across all interpreters.
    // A feature is included if ANY interpreter satisfies its condition.
    // This is safe because build_stable_abi_wheels splits interpreters
    // into abi3-capable vs version-specific groups based on min_version,
    // so interpreters that don't qualify get version-specific wheels.
    let mut seen = HashSet::new();
    for interp in interpreters {
        let env = FeatureConditionEnv {
            major: interp.major,
            minor: interp.minor,
            implementation_name: &interp.implementation_name,
        };
        for feature in FeatureSpec::resolve_conditional(&conditional, &env) {
            if seen.insert(feature.clone()) {
                for &crate_name in crate_names {
                    let prefix = format!("{crate_name}/");
                    if let Some(feat_name) = feature.strip_prefix(&prefix) {
                        extra
                            .entry(crate_name)
                            .or_default()
                            .push(feat_name.to_string());
                    }
                }
            }
        }
    }
    extra
}
