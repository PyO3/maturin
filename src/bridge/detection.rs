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
    cargo_options: &CargoOptions,
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
            let bindings = find_pyo3_bindings(cargo_metadata, deps)?;
            BridgeModel::Bin(bindings)
        } else {
            let bindings =
                find_pyo3_bindings(cargo_metadata, deps)?.context("unknown binding type")?;
            BridgeModel::PyO3(bindings)
        }
    } else {
        match find_pyo3_bindings(cargo_metadata, deps)? {
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
                    BridgeModel::Bin(find_pyo3_bindings(cargo_metadata, deps)?)
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
            let pyo3 = bridge.pyo3().expect("should be pyo3 bindings");
            if !deps.features(lib_name).contains(&"extension-module") {
                let version = &pyo3.version;
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
    // Prefer pyo3 over pyo3-ffi. If pyo3 is present but multi-version, do not
    // fall through to a modern pyo3-ffi and claim support the old pyo3 build
    // script may still lack.
    if deps.contains("pyo3") {
        let Some(pyo3_crate) = deps.get_unambiguous("pyo3") else {
            return Ok(false);
        };
        return Ok(import_lib_support_for(
            cargo_metadata,
            deps,
            "pyo3",
            pyo3_crate,
        ));
    }
    if let Some(pyo3_crate) = deps.get_unambiguous("pyo3-ffi") {
        return Ok(import_lib_support_for(
            cargo_metadata,
            deps,
            "pyo3-ffi",
            pyo3_crate,
        ));
    }
    Ok(false)
}

fn import_lib_support_for(
    cargo_metadata: &Metadata,
    deps: &CrateDependencies,
    lib: &str,
    pyo3_crate: &Node,
) -> bool {
    let pyo3_version = &cargo_metadata[&pyo3_crate.id].version;
    if pyo3_version >= &semver::Version::new(0, 29, 0) {
        return true;
    }
    deps.features(lib)
        .iter()
        .any(|&x| x == "generate-import-lib" || x == "generate-abi3-import-lib")
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

/// Tries to determine the bindings type from dependency.
///
/// Version and package metadata are taken from a single unambiguous dependency
/// node. Multiple remaining versions of `pyo3`/`pyo3-ffi` are an error: a
/// [`PyO3`] value can only represent one package identity.
fn find_pyo3_bindings(
    cargo_metadata: &Metadata,
    deps: &CrateDependencies,
) -> anyhow::Result<Option<PyO3>> {
    if deps.contains("pyo3") {
        let node = deps
            .get_unambiguous("pyo3")
            .with_context(|| ambiguous_bindings_crate_msg("pyo3"))?;
        let package = &cargo_metadata[&node.id];
        let pyo3_metadata = if deps.contains("pyo3-ffi") {
            let pyo3_ffi = deps
                .get_unambiguous("pyo3-ffi")
                .with_context(|| ambiguous_bindings_crate_msg("pyo3-ffi"))?;
            cargo_metadata[&pyo3_ffi.id].metadata.clone()
        } else {
            // Old versions of pyo3 does not depend on pyo3-ffi,
            // thus does not have the metadata
            serde_json::Value::Null
        };
        let metadata = match serde_json::from_value::<Option<PyO3MetadataRaw>>(pyo3_metadata) {
            Ok(Some(metadata)) => Some(metadata.try_into()?),
            Ok(None) | Err(_) => None,
        };
        Ok(Some(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version: package.version.clone(),
            stable_abi: None,
            metadata,
        }))
    } else if deps.contains("pyo3-ffi") {
        let node = deps
            .get_unambiguous("pyo3-ffi")
            .with_context(|| ambiguous_bindings_crate_msg("pyo3-ffi"))?;
        let package = &cargo_metadata[&node.id];
        let metadata =
            match serde_json::from_value::<Option<PyO3MetadataRaw>>(package.metadata.clone()) {
                Ok(Some(metadata)) => Some(metadata.try_into()?),
                Ok(None) | Err(_) => None,
            };
        Ok(Some(PyO3 {
            crate_name: PyO3Crate::PyO3Ffi,
            version: package.version.clone(),
            stable_abi: None,
            metadata,
        }))
    } else {
        Ok(None)
    }
}

fn ambiguous_bindings_crate_msg(name: &str) -> String {
    format!(
        "multiple versions of `{name}` remain in the dependency graph after feature resolution; \
         maturin cannot determine a single bindings version. Ensure the package depends on only \
         one version of `{name}`"
    )
}

/// Return all (transitive) dependency nodes of the *current* crate, keyed by
/// package name. A name maps to several nodes when multiple versions (or
/// source-distinct packages at the same version) are reachable.
///
/// This is different from `metadata.resolve`, which also includes packages
/// that are used in the same workspace, but on which the current crate does not depend.
fn current_crate_dependencies(cargo_metadata: &Metadata) -> Result<HashMap<&str, Vec<&Node>>> {
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

    let mut deps: HashMap<&str, Vec<&Node>> = HashMap::with_capacity(dep_ids.len());
    for (&id, node) in &nodes {
        if !dep_ids.contains(&id) {
            continue;
        }
        let name = cargo_metadata[id].name.as_ref();
        deps.entry(name).or_default().push(node);
    }
    Ok(deps)
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
///
/// Multi-version ambiguity is represented directly: each name maps to every
/// matching resolve node, so callers never need a parallel "is this name
/// ambiguous?" set.
pub struct CrateDependencies<'a> {
    /// Package name -> all root-reachable resolve nodes with that name.
    nodes: HashMap<&'a str, Vec<&'a Node>>,
    /// (Bindings crate name, version) -> features resolved for the current
    /// crate only via `cargo tree`. Empty when verification didn't run.
    scoped_features: HashMap<(String, semver::Version), Vec<String>>,
    cargo_metadata: &'a Metadata,
}

impl<'a> CrateDependencies<'a> {
    pub fn resolve(cargo_metadata: &'a Metadata, cargo_options: &CargoOptions) -> Result<Self> {
        let mut nodes = current_crate_dependencies(cargo_metadata)?;
        let mut scoped_features = HashMap::new();
        if cargo_metadata.workspace_members.len() > 1
            && BINDINGS_CRATES.iter().any(|name| nodes.contains_key(name))
        {
            match scoped_dependency_features(cargo_metadata, cargo_options) {
                Ok(scoped) => {
                    for name in BINDINGS_CRATES {
                        let mut scoped_versions =
                            scoped.keys().filter_map(|(scoped_name, version)| {
                                (scoped_name == name).then_some(version)
                            });
                        match (scoped_versions.next(), scoped_versions.next()) {
                            (None, _) => {
                                // Not actually in the root package's graph.
                                nodes.remove(name);
                            }
                            (Some(version), None) => {
                                // A single scoped version is authoritative only
                                // when exactly one already-retained package id
                                // matches (same name+version from two sources
                                // stays multi-node / ambiguous).
                                if let Some(node) = unique_node_by_version(
                                    cargo_metadata,
                                    nodes.get(name).map(Vec::as_slice).unwrap_or(&[]),
                                    version,
                                ) {
                                    nodes.insert(name, vec![node]);
                                }
                                // else: leave the multi-node entry as-is.
                            }
                            (Some(_), Some(_)) => {
                                // Still several versions after scoping: keep
                                // every metadata node so ambiguity stays visible.
                            }
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
            cargo_metadata,
        })
    }

    fn contains(&self, name: &str) -> bool {
        self.nodes.get(name).is_some_and(|nodes| !nodes.is_empty())
    }

    /// The single resolve node for `name`, or `None` when absent or when
    /// several packages share that name.
    fn get_unambiguous(&self, name: &str) -> Option<&'a Node> {
        match self.nodes.get(name).map(Vec::as_slice)? {
            [node] => Some(*node),
            _ => None,
        }
    }

    /// Features of `name` as resolved for the current crate: the scoped
    /// `cargo tree` resolution when verification ran, the workspace-unified
    /// `cargo metadata` resolution otherwise.
    ///
    /// With several packages of `name`, only features common to every scoped
    /// version are returned (and none if verification didn't run), so abi3 /
    /// extension-module / import-lib checks never trust an arbitrary winner.
    fn features(&self, name: &str) -> Vec<&str> {
        match self.nodes.get(name).map(Vec::as_slice) {
            Some([node]) => {
                if let Some(features) = self.scoped_for(name, node) {
                    features.iter().map(String::as_str).collect()
                } else {
                    node.features.iter().map(AsRef::as_ref).collect()
                }
            }
            Some(nodes) if nodes.len() > 1 => self.common_scoped_features(name),
            _ => Vec::new(),
        }
    }

    /// Intersection of scoped feature sets across every version of `name`.
    fn common_scoped_features(&self, name: &str) -> Vec<&str> {
        let mut versions = self
            .scoped_features
            .iter()
            .filter(|((scoped_name, _), _)| scoped_name == name)
            .map(|(_, features)| features);
        let Some(first) = versions.next() else {
            return Vec::new();
        };
        let mut common: Vec<&str> = first.iter().map(String::as_str).collect();
        for features in versions {
            common.retain(|feature| features.iter().any(|existing| existing == feature));
        }
        common
    }

    /// Scoped features of a specific resolve node, keyed by its package
    /// version so features of another version are never trusted.
    fn scoped_for(&self, name: &str, node: &Node) -> Option<&Vec<String>> {
        let version = &self.cargo_metadata[&node.id].version;
        self.scoped_features
            .get(&(name.to_string(), version.clone()))
    }
}

/// Among `candidates`, return the unique node whose package version equals
/// `version`. `None` when zero or several match.
fn unique_node_by_version<'a>(
    cargo_metadata: &Metadata,
    candidates: &[&'a Node],
    version: &semver::Version,
) -> Option<&'a Node> {
    let mut matches = candidates
        .iter()
        .copied()
        .filter(|node| &cargo_metadata[&node.id].version == version);
    match (matches.next(), matches.next()) {
        (Some(node), None) => Some(node),
        _ => None,
    }
}

/// Collect the bindings crates in the root package's dependency graph and
/// their active features as resolved by `cargo tree`, i.e. with features
/// resolved for the root package only instead of unified across the workspace.
fn scoped_dependency_features(
    cargo_metadata: &Metadata,
    cargo_options: &CargoOptions,
) -> Result<HashMap<(String, semver::Version), Vec<String>>> {
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
    cmd.args(cargo_options.cargo_tree_args());
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
    let mut scoped: HashMap<(String, semver::Version), Vec<String>> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // A line looks like `pyo3 v0.29.0|abi3,default,macros` (path/git
        // dependencies append their source in parentheses), with ` (*)`
        // appended to de-duplicated repeats of a subtree.
        let line = line.trim_end().trim_end_matches(" (*)");
        let Some((package, features)) = line.split_once('|') else {
            continue;
        };
        let mut package_parts = package.split_whitespace();
        let Some(name) = package_parts.next() else {
            continue;
        };
        if !BINDINGS_CRATES.contains(&name) {
            continue;
        }
        // Key by version so that two versions of the same crate don't get
        // their feature sets unioned into one. A bindings crate line whose
        // version doesn't parse means the `{p}` format changed; erroring makes
        // the caller fall back to the unified resolution instead of treating
        // the crate as absent.
        let version = package_parts
            .next()
            .and_then(|v| v.strip_prefix('v'))
            .and_then(|v| semver::Version::parse(v).ok())
            .with_context(|| format!("unexpected `cargo tree` output line: {line}"))?;
        let entry = scoped.entry((name.to_string(), version)).or_default();
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
