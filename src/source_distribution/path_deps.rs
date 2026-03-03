use anyhow::{Context, Result};
use cargo_metadata::{Metadata, MetadataCommand, PackageId};
use std::collections::HashMap;
use std::path::PathBuf;

/// Path dependency information.
/// It may be in a different workspace than the root crate.
///
/// ```toml
/// [dependencies]
/// foo = { path = "path/to/foo" }
/// ```
#[derive(Debug, Clone)]
pub struct PathDependency {
    /// `Cargo.toml` path of the path dependency
    pub(super) manifest_path: PathBuf,
    /// workspace root of the path dependency
    pub(super) workspace_root: PathBuf,
    /// readme path of the path dependency
    pub(super) readme: Option<PathBuf>,
    /// license-file path of the path dependency
    pub(super) license_file: Option<PathBuf>,
    /// Resolved package metadata from `cargo metadata`, used to inline
    /// workspace-inherited fields when the workspace manifest is outside the sdist root.
    pub(super) resolved_package: Option<cargo_metadata::Package>,
}

/// Finds all path dependencies of the crate.
///
/// Walks the resolved dependency graph from `cargo metadata` and collects
/// every transitively-reachable path dependency.  For same-workspace deps
/// the root metadata already contains all resolved data (workspace root,
/// package fields, readme, license-file), so no extra subprocess is needed.
/// Only cross-workspace path deps require a separate `cargo metadata` call
/// to discover their workspace root.
pub fn find_path_deps(cargo_metadata: &Metadata) -> Result<HashMap<String, PathDependency>> {
    let root = cargo_metadata
        .root_package()
        .context("Expected the dependency graph to have a root package")?;

    let workspace_root = &cargo_metadata.workspace_root;

    // Pre-build lookup indices to avoid repeated linear scans
    let packages_by_id: HashMap<&PackageId, &cargo_metadata::Package> =
        cargo_metadata.packages.iter().map(|p| (&p.id, p)).collect();
    let resolve_nodes: HashMap<&PackageId, &[cargo_metadata::NodeDep]> = cargo_metadata
        .resolve
        .as_ref()
        .context("cargo metadata is missing dependency resolve information")?
        .nodes
        .iter()
        .map(|node| (&node.id, node.deps.as_slice()))
        .collect();

    // Scan the dependency graph for path dependencies
    let mut path_deps = HashMap::new();
    let mut stack: Vec<&cargo_metadata::Package> = vec![root];
    while let Some(top) = stack.pop() {
        let node_deps = resolve_nodes
            .get(&top.id)
            .with_context(|| format!("missing resolve node for package {}", top.id))?;
        for node_dep in *node_deps {
            let dep_pkg = packages_by_id
                .get(&node_dep.pkg)
                .with_context(|| format!("missing package metadata for {}", node_dep.pkg))?;
            // Match the resolved dependency back to the declared dependency
            // to check if it's a path dependency.
            let dependency = top
                .dependencies
                .iter()
                .find(|d| d.name == dep_pkg.name.as_ref())
                .with_context(|| {
                    format!(
                        "could not find dependency {} in package {}",
                        dep_pkg.name, top.id
                    )
                })?;
            if let Some(path) = &dependency.path {
                let dep_name = dependency.rename.as_ref().unwrap_or(&dependency.name);
                if path_deps.contains_key(dep_name) {
                    continue;
                }
                let dep_manifest_path = path.join("Cargo.toml");

                // For same-workspace path deps, the root cargo metadata already
                // contains everything we need: workspace root, resolved package
                // fields (including workspace-inherited ones), readme, and
                // license-file.  Only cross-workspace deps require a separate
                // `cargo metadata` invocation to discover their workspace root.
                let is_same_workspace = dep_manifest_path.starts_with(workspace_root);
                let dep_workspace_root = if is_same_workspace {
                    workspace_root.clone().into_std_path_buf()
                } else {
                    let path_dep_metadata = MetadataCommand::new()
                        .manifest_path(&dep_manifest_path)
                        .verbose(true)
                        // We only need the workspace root, not the dep graph
                        .no_deps()
                        .exec()
                        .with_context(|| {
                            format!(
                                "Failed to resolve workspace root for {} at '{dep_manifest_path}'",
                                node_dep.pkg
                            )
                        })?;
                    path_dep_metadata.workspace_root.into_std_path_buf()
                };

                // The root cargo metadata already resolves all package fields
                // (including workspace-inherited ones) for every package in the
                // dependency graph, regardless of which workspace they belong to.
                // Only cross-workspace deps need the resolved package stored, as
                // it's used to inline workspace-inherited fields when the workspace
                // manifest falls outside the sdist root.
                path_deps.insert(
                    dep_name.clone(),
                    PathDependency {
                        manifest_path: dep_manifest_path.into_std_path_buf(),
                        workspace_root: dep_workspace_root,
                        readme: dep_pkg
                            .readme
                            .as_ref()
                            .map(|r| r.clone().into_std_path_buf()),
                        license_file: dep_pkg
                            .license_file
                            .as_ref()
                            .map(|l| l.clone().into_std_path_buf()),
                        resolved_package: if is_same_workspace {
                            None
                        } else {
                            Some((*dep_pkg).clone())
                        },
                    },
                );
                // Continue scanning the path dependency's own dependencies
                if let Some(&dep_package) = packages_by_id.get(&node_dep.pkg) {
                    stack.push(dep_package)
                }
            }
        }
    }
    Ok(path_deps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo_metadata::MetadataCommand;
    use fs_err as fs;
    use std::path::Path;

    #[test]
    fn test_find_path_deps_captures_workspace_license_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let workspace_root = temp_dir.path();
        let py_dir = workspace_root.join("py");
        let dep_dir = workspace_root.join("dep");

        fs::create_dir_all(py_dir.join("src")).unwrap();
        fs::create_dir_all(dep_dir.join("src")).unwrap();
        fs::write(py_dir.join("src/lib.rs"), "").unwrap();
        fs::write(dep_dir.join("src/lib.rs"), "").unwrap();
        fs::write(workspace_root.join("LICENSE"), "MIT").unwrap();

        fs::write(
            workspace_root.join("Cargo.toml"),
            indoc::indoc!(
                r#"
                [workspace]
                resolver = "2"
                members = ["py", "dep"]

                [workspace.package]
                license-file = "LICENSE"
                "#
            ),
        )
        .unwrap();

        fs::write(
            dep_dir.join("Cargo.toml"),
            indoc::indoc!(
                r#"
                [package]
                name = "dep"
                version = "0.1.0"
                edition = "2021"
                license-file.workspace = true
                "#
            ),
        )
        .unwrap();

        fs::write(
            py_dir.join("Cargo.toml"),
            indoc::indoc!(
                r#"
                [package]
                name = "py"
                version = "0.1.0"
                edition = "2021"

                [dependencies]
                dep = { path = "../dep" }
                "#
            ),
        )
        .unwrap();

        let cargo_metadata = MetadataCommand::new()
            .manifest_path(py_dir.join("Cargo.toml"))
            .exec()
            .unwrap();

        let path_deps = find_path_deps(&cargo_metadata).unwrap();
        let dep = path_deps.get("dep").expect("missing path dependency");
        assert_eq!(dep.license_file.as_deref(), Some(Path::new("../LICENSE")));
    }
}
