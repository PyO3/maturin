//! Bridge model detection from cargo metadata.
//!
//! This module inspects the dependency graph and feature flags of a crate
//! to determine which binding model (pyo3, cffi, uniffi, bin) to use,
//! whether abi3 is enabled, and whether `generate-import-lib` is active.

use super::{
    ABI3T_MINIMUM_PYTHON_MINOR, BridgeModel, PyO3, PyO3Crate, PyO3MetadataRaw, StableAbi,
    StableAbiKind, StableAbiVersion,
};
use crate::pyproject_toml::{FeatureConditionEnv, FeatureSpec};
use crate::{CargoOptions, PyProjectToml};
use anyhow::{Context, Result, bail};
use cargo_metadata::{CrateType, Metadata, Node, PackageId, TargetKind};
use std::collections::{HashMap, HashSet};
use std::process::Command;

// pyo3-ffi is ordered first because it is newer and more restrictive.
const PYO3_BINDING_CRATES: [PyO3Crate; 2] = [PyO3Crate::PyO3Ffi, PyO3Crate::PyO3];

/// Tries to determine the [`BridgeModel`] for the target crate.
///
/// Inspects cargo metadata to detect pyo3/cffi/uniffi bindings, abi3 support,
/// and extension-module feature usage. If `bridge` is `Some`, the binding type
/// is forced; otherwise it's auto-detected from dependencies and target types.
///
/// Conditional pyo3/pyo3-ffi features from pyproject.toml are excluded from
/// abi3 inference here. Use [`upgrade_bridge_stable_abi`] after interpreter resolution
/// to evaluate them.
pub fn find_bridge(
    cargo_metadata: &Metadata,
    bridge: Option<&str>,
    cargo_options: Option<&CargoOptions>,
) -> Result<BridgeModel> {
    let deps = CrateDependencies::resolve(cargo_metadata, cargo_options)?;
    find_bridge_with_deps(cargo_metadata, bridge, &deps)
}

/// [`find_bridge`] with an already-resolved dependency graph, so callers that
/// also need [`CrateDependencies`] elsewhere resolve it only once.
pub fn find_bridge_with_deps(
    cargo_metadata: &Metadata,
    bridge: Option<&str>,
    deps: &CrateDependencies,
) -> Result<BridgeModel> {
    let no_extra_features = HashMap::new();
    let packages: HashMap<&str, &cargo_metadata::Package> = cargo_metadata
        .packages
        .iter()
        .filter_map(|pkg| {
            let name = pkg.name.as_ref();
            BINDINGS_CRATES.contains(&name).then_some((name, pkg))
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
            let bindings = find_pyo3_bindings(deps, &packages)?;
            BridgeModel::Bin(bindings)
        } else {
            let bindings = find_pyo3_bindings(deps, &packages)?.context("unknown binding type")?;
            BridgeModel::PyO3(bindings)
        }
    } else {
        match find_pyo3_bindings(deps, &packages)? {
            Some(bindings) => {
                if !targets.contains(&CrateType::CDyLib) && targets.contains(&CrateType::Bin) {
                    BridgeModel::Bin(Some(bindings))
                } else {
                    BridgeModel::PyO3(bindings)
                }
            }
            _ => {
                if deps.contains("uniffi") {
                    BridgeModel::UniFfi
                } else if targets.contains(&CrateType::CDyLib) {
                    BridgeModel::Cffi
                } else if targets.contains(&CrateType::Bin) {
                    BridgeModel::Bin(find_pyo3_bindings(deps, &packages)?)
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
            let pyo3_node = deps
                .get(lib_name)
                .expect("pyo3 bridge crate should be in the dependency graph");
            if !deps.features(lib_name).contains(&"extension-module") {
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

            return if let Some(stable_abi) = has_stable_abi(deps, &no_extra_features, &[])? {
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

/// Select the stable ABI for a bridge model after interpreter resolution.
///
/// This is the second phase of bridge detection: [`find_bridge`] excludes
/// conditional features and picks a conservative default, then after
/// interpreter resolution this function re-checks plain and conditional
/// stable ABI features and chooses the single stable ABI family this build
/// should attempt.
pub fn upgrade_bridge_stable_abi(
    bridge: BridgeModel,
    deps: &CrateDependencies,
    pyproject: Option<&PyProjectToml>,
    interpreters: &[crate::PythonInterpreter],
) -> Result<BridgeModel> {
    // Only relevant for pyo3 bridges
    let Some(pyo3) = bridge.pyo3() else {
        return Ok(bridge);
    };

    let extra_pyo3_features = pyo3_features_from_conditional(pyproject, interpreters);
    if let Some(stable_abi) = has_stable_abi(deps, &extra_pyo3_features, interpreters)? {
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
pub fn has_windows_import_lib_support(
    cargo_metadata: &Metadata,
    deps: &CrateDependencies,
) -> Result<bool> {
    for &lib in PYO3_BINDING_CRATES.iter().rev() {
        let lib = lib.as_str();
        if let Some(pyo3_crate) = deps.get(lib) {
            let pyo3_version = &cargo_metadata[&pyo3_crate.id].version;
            if pyo3_version >= &semver::Version::new(0, 29, 0) {
                return Ok(true);
            }
            let generate_import_lib = deps
                .features(lib)
                .iter()
                .any(|&x| x == "generate-import-lib" || x == "generate-abi3-import-lib");
            return Ok(generate_import_lib);
        }
    }
    Ok(false)
}

fn has_stable_abi(
    deps: &CrateDependencies,
    extra_features: &HashMap<&str, Vec<String>>,
    interpreters: &[crate::PythonInterpreter],
) -> Result<Option<StableAbi>> {
    let abi3t = has_stable_abi_from_kind(deps, extra_features, StableAbiKind::Abi3t)?;
    let abi3 = has_stable_abi_from_kind(deps, extra_features, StableAbiKind::Abi3)?;

    let selected = [abi3t, abi3].into_iter().flatten().find(|stable_abi| {
        interpreters.iter().any(|interpreter| {
            interpreter.has_stable_api(stable_abi.kind)
                && stable_abi
                    .version
                    .min_version()
                    .is_none_or(|(major, minor)| {
                        (interpreter.major as u8, interpreter.minor as u8) >= (major, minor)
                    })
        })
    });

    // If no resolved interpreter can use either stable ABI, keep abi3 as the
    // conservative project marker when available; the build will fall back to
    // version-specific wheels for the non-matching interpreters.
    Ok(selected.or(abi3).or(abi3t))
}

/// pyo3 supports building stable abi wheels if the unstable-api feature is not selected
fn has_stable_abi_from_kind(
    deps: &CrateDependencies,
    extra_features: &HashMap<&str, Vec<String>>,
    abi_kind: StableAbiKind,
) -> Result<Option<StableAbi>> {
    for &lib in PYO3_BINDING_CRATES.iter() {
        let lib = lib.as_str();
        if deps.contains(lib) {
            let extra = extra_features.get(lib);
            // Find the minimal stable abi python version. If there is none, stable abi hasn't been selected
            // This parses abi3-py{major}{minor} and returns the minimal (major, minor) tuple
            let all_features: Vec<&str> = deps
                .features(lib)
                .into_iter()
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
                    let (major, minor) = if abi_kind == StableAbiKind::Abi3t {
                        (major, minor).max((3, ABI3T_MINIMUM_PYTHON_MINOR))
                    } else {
                        (major, minor)
                    };
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
    deps: &CrateDependencies,
    packages: &HashMap<&str, &cargo_metadata::Package>,
) -> anyhow::Result<Option<PyO3>> {
    if deps.contains("pyo3") {
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
    } else if deps.contains("pyo3-ffi") {
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

/// Names of the crates whose presence in the dependency graph drives bindings
/// auto-detection.
const BINDINGS_CRATES: [&str; 3] = ["pyo3", "pyo3-ffi", "uniffi"];

/// The (transitive) dependencies of the current crate, with features resolved
/// for the current crate only.
///
/// `cargo metadata` unifies features across all workspace members
/// (rust-lang/cargo#7754), while `cargo build -p <root>` resolves features for
/// the selected package only. In a multi-member workspace this makes the
/// unified resolve graph over-report: an optional pyo3 dependency that only a
/// sibling member enables shows up in the root package's graph (#3256), and a
/// pyo3 feature like `abi3` that only a sibling enables shows up on the pyo3
/// node (#876), even though a scoped build never enables either.
///
/// When that discrepancy is possible (multi-member workspace with a bindings
/// crate in the unified graph), the resolver verifies against `cargo tree`,
/// which resolves features the same way a scoped build does: bindings crates
/// that aren't really in the graph are dropped, and [`Self::features`] prefers
/// the scoped feature resolution over the unified one.
pub struct CrateDependencies<'a> {
    nodes: HashMap<&'a str, &'a Node>,
    /// Bindings crate name -> features resolved for the current crate only
    /// via `cargo tree`. Empty when verification didn't run.
    scoped_features: HashMap<String, Vec<String>>,
}

impl<'a> CrateDependencies<'a> {
    pub fn resolve(
        cargo_metadata: &'a Metadata,
        cargo_options: Option<&CargoOptions>,
    ) -> Result<Self> {
        let mut nodes = current_crate_dependencies(cargo_metadata)?;
        let mut scoped_features = HashMap::new();
        if cargo_metadata.workspace_members.len() > 1
            && BINDINGS_CRATES.iter().any(|name| nodes.contains_key(name))
        {
            match scoped_dependency_features(cargo_metadata, cargo_options) {
                Ok(scoped) => {
                    for name in BINDINGS_CRATES {
                        if !scoped.contains_key(name) {
                            nodes.remove(name);
                        }
                    }
                    scoped_features = scoped;
                }
                Err(err) => {
                    eprintln!(
                        "⚠️  Warning: Failed to verify the dependency graph with `cargo tree`, \
                         bindings detection may mistake feature-gated dependencies of other \
                         workspace members as enabled: {err:#}"
                    );
                }
            }
        }
        Ok(Self {
            nodes,
            scoped_features,
        })
    }

    fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    fn get(&self, name: &str) -> Option<&'a Node> {
        self.nodes.get(name).copied()
    }

    /// Features of `name` as resolved for the current crate: the scoped
    /// `cargo tree` resolution when verification ran, the workspace-unified
    /// `cargo metadata` resolution otherwise.
    fn features(&self, name: &str) -> Vec<&str> {
        if let Some(features) = self.scoped_features.get(name) {
            features.iter().map(String::as_str).collect()
        } else {
            self.nodes
                .get(name)
                .map(|node| node.features.iter().map(AsRef::as_ref).collect())
                .unwrap_or_default()
        }
    }
}

/// Collect the bindings crates in the root package's dependency graph and
/// their active features as resolved by `cargo tree`, i.e. with features
/// resolved for the root package only instead of unified across the workspace.
fn scoped_dependency_features(
    cargo_metadata: &Metadata,
    cargo_options: Option<&CargoOptions>,
) -> Result<HashMap<String, Vec<String>>> {
    let root_package = cargo_metadata
        .root_package()
        .context("Expected cargo to return metadata with root_package")?;
    let mut cmd = Command::new("cargo");
    cmd.arg("tree")
        .arg("--manifest-path")
        .arg(root_package.manifest_path.as_std_path())
        .args([
            "--edges",
            "normal,build,dev",
            "--prefix",
            "none",
            "--format",
            "{p}|{f}",
            "--color",
            "never",
        ]);
    match cargo_options {
        Some(options) => cmd.args(options.cargo_tree_args()),
        // Match `cargo metadata`'s platform-unfiltered resolve
        None => cmd.args(["--target", "all"]),
    };
    let output = cmd
        .output()
        .context("Failed to run `cargo tree`. Do you have cargo in your PATH?")?;
    if !output.status.success() {
        bail!(
            "`cargo tree` exited with {}:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
        );
    }
    // With multiple `--target` flags (universal2) cargo tree prints one graph
    // per target, so union the features per package across all of them, like
    // `cargo metadata` does with multiple `--filter-platform` flags.
    let mut scoped: HashMap<String, Vec<String>> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // A line looks like `pyo3 v0.29.0|abi3,default,macros`, with
        // ` (*)` appended to de-duplicated repeats of a subtree.
        let line = line.trim_end().trim_end_matches(" (*)");
        let Some((package, features)) = line.split_once('|') else {
            continue;
        };
        let Some(name) = package.split_whitespace().next() else {
            continue;
        };
        if !BINDINGS_CRATES.contains(&name) {
            continue;
        }
        let entry = scoped.entry(name.to_string()).or_default();
        for feature in features.split(',').filter(|feature| !feature.is_empty()) {
            if !entry.iter().any(|existing| existing == feature) {
                entry.push(feature.to_string());
            }
        }
    }
    Ok(scoped)
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
