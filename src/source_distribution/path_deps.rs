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

/// Finds all path dependencies of the crate
pub fn find_path_deps(cargo_metadata: &Metadata) -> Result<HashMap<String, PathDependency>> {
    let root = cargo_metadata
        .root_package()
        .context("Expected the dependency graph to have a root package")?;

    // Pre-build lookup indices to avoid repeated linear scans
    let packages_by_id: HashMap<&PackageId, &cargo_metadata::Package> =
        cargo_metadata.packages.iter().map(|p| (&p.id, p)).collect();
    let pkg_readmes: HashMap<&PackageId, PathBuf> = cargo_metadata
        .packages
        .iter()
        .filter_map(|package| {
            package
                .readme
                .as_ref()
                .map(|readme| (&package.id, readme.clone().into_std_path_buf()))
        })
        .collect();
    let pkg_license_files: HashMap<&PackageId, PathBuf> = cargo_metadata
        .packages
        .iter()
        .filter_map(|package| {
            package
                .license_file
                .as_ref()
                .map(|license_file| (&package.id, license_file.clone().into_std_path_buf()))
        })
        .collect();
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
                // Path dependencies may not be in the same workspace as the root crate,
                // thus we need to find out its workspace root from `cargo metadata`
                let path_dep_metadata = MetadataCommand::new()
                    .manifest_path(&dep_manifest_path)
                    .verbose(true)
                    // We don't need to resolve the dependency graph
                    .no_deps()
                    .exec()
                    .with_context(|| {
                        format!(
                            "Failed to resolve workspace root for {} at '{dep_manifest_path}'",
                            node_dep.pkg
                        )
                    })?;

                let resolved_package = path_dep_metadata
                    .packages
                    .iter()
                    .find(|p| p.manifest_path == dep_manifest_path)
                    .cloned()
                    .with_context(|| {
                        format!(
                            "Failed to find package for {} in cargo metadata",
                            dep_manifest_path
                        )
                    })?;
                path_deps.insert(
                    dep_name.clone(),
                    PathDependency {
                        manifest_path: PathBuf::from(dep_manifest_path.clone()),
                        workspace_root: path_dep_metadata
                            .workspace_root
                            .clone()
                            .into_std_path_buf(),
                        readme: pkg_readmes.get(&node_dep.pkg).cloned(),
                        license_file: pkg_license_files.get(&node_dep.pkg).cloned(),
                        resolved_package: Some(resolved_package),
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
