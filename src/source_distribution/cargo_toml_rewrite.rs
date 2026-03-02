use super::PathDependency;
use super::utils::{normalize_path, relative_path};
use anyhow::{Context, Result};
use fs_err as fs;
use path_slash::PathExt as _;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use toml_edit::DocumentMut;
use tracing::debug;

pub(super) fn parse_toml_file(path: &Path, kind: &str) -> Result<toml_edit::DocumentMut> {
    let text = fs::read_to_string(path)?;
    let document = text
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("Failed to parse {} at {}", kind, path.display()))?;
    Ok(document)
}

/// Rewrite Cargo.toml to only retain path dependencies that are actually used
///
/// We only want to add path dependencies that are actually used
/// to reduce the size of the source distribution.
pub(super) fn rewrite_cargo_toml(
    document: &mut DocumentMut,
    manifest_path: &Path,
    known_path_deps: &HashMap<String, PathDependency>,
) -> Result<()> {
    debug!(
        "Rewriting Cargo.toml `workspace.members` at {}",
        manifest_path.display()
    );
    // Update workspace members
    if let Some(workspace) = document.get_mut("workspace").and_then(|x| x.as_table_mut())
        && let Some(members) = workspace.get_mut("members").and_then(|x| x.as_array())
    {
        if known_path_deps.is_empty() {
            // Remove workspace members when there isn't any path dep
            workspace.remove("members");
            if workspace.is_empty() {
                // Remove workspace all together if it's empty
                document.remove("workspace");
            }
        } else {
            // Build a set of relative directory paths (from workspace root) for
            // all known path dependencies. Workspace `members` entries are
            // directory paths, not crate names, so we must compare against the
            // actual directory of each dependency rather than its name.
            let relative_dep_dirs: HashSet<String> = known_path_deps
                .values()
                .filter_map(|path_dep| {
                    let manifest_rel = path_dep
                        .manifest_path
                        .strip_prefix(&path_dep.workspace_root)
                        .ok()?;
                    // Strip the trailing `Cargo.toml` to get the directory
                    manifest_rel.parent().and_then(|p| p.to_slash()).map(|s| {
                        if s.is_empty() {
                            ".".into()
                        } else {
                            s.into_owned()
                        }
                    })
                })
                .collect();

            let mut new_members = toml_edit::Array::new();
            for member in members {
                if let toml_edit::Value::String(s) = member {
                    let member_path = s.value();
                    // See https://github.com/rust-lang/cargo/blob/0de91c89e6479016d0ed8719fdc2947044335b36/src/cargo/util/restricted_names.rs#L119-L122
                    let is_glob_pattern = member_path.contains(['*', '?', '[', ']']);
                    if is_glob_pattern {
                        let pattern = glob::Pattern::new(member_path).with_context(|| {
                            format!(
                                "Invalid `workspace.members` glob pattern: {} in {}",
                                member_path,
                                manifest_path.display()
                            )
                        })?;
                        if relative_dep_dirs.iter().any(|dir| pattern.matches(dir)) {
                            new_members.push(member_path);
                        }
                    } else if relative_dep_dirs.contains(member_path) {
                        new_members.push(member_path);
                    }
                }
            }
            if !new_members.is_empty() {
                workspace["members"] = toml_edit::value(new_members);
            } else {
                workspace.remove("members");
            }
        }
    }

    // Remove `default-members` to avoid build failures when some entries
    // are not included in the sdist. Without `default-members`, Cargo
    // treats all `members` as defaults, which is the correct behavior
    // for source distributions. See https://github.com/PyO3/maturin/issues/2046
    if let Some(workspace) = document.get_mut("workspace").and_then(|x| x.as_table_mut()) {
        workspace.remove("default-members");
    }

    Ok(())
}

/// Strip all non-workspace tables from a workspace Cargo.toml.
///
/// When the workspace root's Cargo.toml is also a `[package]` (i.e. it's not a
/// virtual workspace), the package's source files are typically not included in
/// the sdist. This function strips everything except workspace-level tables
/// (`[workspace]`, `[profile]`, `[patch]`, `[replace]`) so Cargo treats it as
/// a virtual workspace.
pub(super) fn strip_non_workspace_tables(document: &mut DocumentMut, manifest_path: &Path) {
    debug!(
        "Stripping [package] from workspace Cargo.toml at {} (source files not in sdist)",
        manifest_path.display()
    );
    let package_level_keys: Vec<String> = document
        .as_table()
        .iter()
        .filter(|(key, _)| !matches!(&**key, "workspace" | "profile" | "patch" | "replace"))
        .map(|(key, _)| key.to_string())
        .collect();
    for key in &package_level_keys {
        document.remove(key);
    }
}

/// Inlines workspace-inherited fields in a path dependency's `Cargo.toml`
/// using resolved values from `cargo metadata`.
///
/// This is needed when the dependency's workspace manifest falls outside the
/// sdist root (e.g. a crate excluded from a parent workspace that depends on
/// sibling workspace members).  Without this, `field.workspace = true` entries
/// would fail to resolve when building from the sdist.
pub(super) fn resolve_workspace_inheritance(
    document: &mut DocumentMut,
    resolved: &cargo_metadata::Package,
) {
    // Resolve `[package]` fields that support `field.workspace = true`
    if let Some(package) = document.get_mut("package").and_then(|p| p.as_table_mut()) {
        let version_str = resolved.version.to_string();
        let edition_str = resolved.edition.to_string();
        let rust_version_str = resolved.rust_version.as_ref().map(|v| v.to_string());
        let string_fields: &[(&str, Option<&str>)] = &[
            ("version", Some(&version_str)),
            ("edition", Some(&edition_str)),
            ("description", resolved.description.as_deref()),
            ("license", resolved.license.as_deref()),
            ("repository", resolved.repository.as_deref()),
            ("homepage", resolved.homepage.as_deref()),
            ("documentation", resolved.documentation.as_deref()),
            ("rust-version", rust_version_str.as_deref()),
        ];

        for (key, value) in string_fields {
            if is_workspace_inherited(package, key) {
                if let Some(val) = value {
                    package.insert(key, toml_edit::value(*val));
                } else {
                    package.remove(key);
                }
            }
        }

        // Handle array fields
        let array_fields: &[(&str, &[String])] = &[
            ("authors", &resolved.authors),
            ("keywords", &resolved.keywords),
            ("categories", &resolved.categories),
        ];

        for (key, values) in array_fields {
            if is_workspace_inherited(package, key) {
                if values.is_empty() {
                    package.remove(key);
                } else {
                    let mut arr = toml_edit::Array::new();
                    for v in *values {
                        arr.push(v.as_str());
                    }
                    package.insert(key, toml_edit::value(arr));
                }
            }
        }

        // `readme` and `license-file` are NOT inlined here because they need
        // special handling: the file is copied into the sdist next to Cargo.toml
        // and the path is rewritten to just the filename by the caller
        // (`resolve_and_add_manifest_asset` + `rewrite_cargo_toml_package_field`).
        // We only need to remove the `workspace = true` marker so cargo doesn't
        // try to look it up from a workspace that no longer exists.
        for key in ["readme", "license-file"] {
            if is_workspace_inherited(package, key) {
                package.remove(key);
            }
        }
    }

    // Resolve `workspace = true` in dependency tables
    resolve_workspace_deps(document, "dependencies", resolved);
    resolve_workspace_deps(document, "dev-dependencies", resolved);
    resolve_workspace_deps(document, "build-dependencies", resolved);

    // Handle `[target.'cfg(...)'.dependencies]` etc.
    if let Some(target) = document.get_mut("target").and_then(|t| t.as_table_mut()) {
        let target_keys: Vec<String> = target.iter().map(|(k, _)| k.to_string()).collect();
        for target_key in target_keys {
            let Some(target_val) = target.get_mut(&target_key) else {
                continue;
            };
            for dep_kind in &["dependencies", "dev-dependencies", "build-dependencies"] {
                let Some(deps) = target_val.get_mut(*dep_kind).and_then(|t| t.as_table_mut())
                else {
                    continue;
                };
                let dep_names: Vec<String> = deps.iter().map(|(k, _)| k.to_string()).collect();
                for dep_name in dep_names {
                    let is_workspace = deps
                        .get(&dep_name)
                        .and_then(|d| d.as_table_like())
                        .and_then(|t| t.get("workspace"))
                        .and_then(|v| v.as_bool())
                        == Some(true);
                    if !is_workspace {
                        continue;
                    }
                    if let Some(resolved_dep) =
                        find_resolved_dep(resolved, &dep_name, Some(&target_key))
                    {
                        deps.insert(&dep_name, resolved_dep_to_toml(&resolved_dep));
                    } else {
                        debug!(
                            "Could not resolve workspace dependency {dep_name} in \
                             target.{target_key}.{dep_kind}"
                        );
                    }
                }
            }
        }
    }
}

/// Returns `true` if a `[package]` field has the form `field.workspace = true`.
fn is_workspace_inherited(package: &toml_edit::Table, key: &str) -> bool {
    package
        .get(key)
        .and_then(|v| v.as_table_like())
        .and_then(|t| t.get("workspace"))
        .and_then(|v| v.as_bool())
        == Some(true)
}

/// Resolves `workspace = true` entries in a `[dependencies]`-style table.
fn resolve_workspace_deps(
    document: &mut DocumentMut,
    dep_kind: &str,
    resolved: &cargo_metadata::Package,
) {
    let dep_names: Vec<String> = document
        .get(dep_kind)
        .and_then(|t| t.as_table())
        .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default();

    for dep_name in dep_names {
        let is_workspace = document
            .get(dep_kind)
            .and_then(|t| t.get(&dep_name))
            .and_then(|d| d.as_table_like())
            .and_then(|t| t.get("workspace"))
            .and_then(|v| v.as_bool())
            == Some(true);

        if !is_workspace {
            continue;
        }

        if let Some(resolved_dep) = find_resolved_dep(resolved, &dep_name, None) {
            let new_entry = resolved_dep_to_toml(&resolved_dep);
            if let Some(deps_table) = document.get_mut(dep_kind).and_then(|t| t.as_table_mut()) {
                deps_table.insert(&dep_name, new_entry);
            }
        } else {
            debug!("Could not resolve workspace dependency {dep_name} in [{dep_kind}]");
        }
    }
}

/// Finds a resolved dependency by name in the package metadata.
fn find_resolved_dep(
    resolved: &cargo_metadata::Package,
    name: &str,
    target: Option<&str>,
) -> Option<ResolvedDep> {
    resolved.dependencies.iter().find_map(|d| {
        // Match by name, considering renames
        let matches_name = d.rename.as_deref() == Some(name) || d.name == name;
        if !matches_name {
            return None;
        }
        // If a target is specified, match against it
        if let Some(target_str) = target
            && d.target.as_ref().map(|t| t.to_string()).as_deref() != Some(target_str)
        {
            return None;
        }
        // When the dep is renamed, `d.rename` is the alias (the Cargo.toml key)
        // and `d.name` is the real package name. We need `package = "<real_name>"`.
        let package = d.rename.as_ref().map(|_| d.name.clone());
        // For path deps, compute the relative path from the dependent's manifest dir
        let path = d.path.as_ref().map(|dep_path| {
            let manifest_dir = resolved.manifest_path.parent().unwrap();
            relative_path(manifest_dir.as_std_path(), dep_path.as_std_path())
        });
        Some(ResolvedDep {
            req: d.req.to_string(),
            optional: d.optional,
            default_features: d.uses_default_features,
            features: d.features.clone(),
            package,
            path,
        })
    })
}

struct ResolvedDep {
    req: String,
    optional: bool,
    default_features: bool,
    features: Vec<String>,
    /// When the dep is renamed (aliased), the real package name.
    /// Emitted as `package = "<name>"` in Cargo.toml.
    package: Option<String>,
    /// For path dependencies, the relative path from the dependent crate
    /// to the dependency directory.
    path: Option<PathBuf>,
}

/// Converts a resolved dependency into a TOML value for Cargo.toml.
fn resolved_dep_to_toml(dep: &ResolvedDep) -> toml_edit::Item {
    // Simple case: just a version string (only for non-path, non-renamed deps)
    if dep.path.is_none()
        && dep.default_features
        && !dep.optional
        && dep.features.is_empty()
        && dep.package.is_none()
    {
        return toml_edit::value(&dep.req);
    }

    let mut table = toml_edit::InlineTable::new();
    if let Some(path) = &dep.path {
        let path_str = path.to_slash().unwrap_or_else(|| path.to_string_lossy());
        table.insert("path", path_str.as_ref().into());
    } else {
        table.insert("version", dep.req.as_str().into());
    }

    if !dep.default_features {
        table.insert("default-features", false.into());
    }
    if dep.optional {
        table.insert("optional", true.into());
    }
    if !dep.features.is_empty() {
        let mut arr = toml_edit::Array::new();
        for f in &dep.features {
            arr.push(f.as_str());
        }
        table.insert("features", toml_edit::Value::Array(arr));
    }
    if let Some(package) = &dep.package {
        table.insert("package", package.as_str().into());
    }

    toml_edit::value(toml_edit::Value::InlineTable(table))
}

// Strip targets whose source files are excluded from the sdist, matching Cargo's
// behavior when `package.include`/`package.exclude` or tool.maturin excludes remove them.
pub(super) fn rewrite_cargo_toml_targets(
    document: &mut DocumentMut,
    manifest_path: &Path,
    packaged_files: &HashSet<PathBuf>,
) -> Result<()> {
    debug!(
        "Rewriting Cargo.toml build targets at {}",
        manifest_path.display()
    );

    let manifest_dir = manifest_path.parent().unwrap();
    let package_name = document
        .get("package")
        .and_then(|item| item.as_table())
        .and_then(|table| table.get("name"))
        .and_then(|item| item.as_str())
        .map(str::to_string);

    // We need to normalize paths without accessing the filesystem (which might not match
    // the manifest context) and without resolving symlinks. This matches `cargo package --list`
    // behavior which outputs normalized paths.
    let normalize = |path: &Path| -> PathBuf {
        let path = if path.is_absolute() {
            path.strip_prefix(manifest_dir).unwrap_or(path)
        } else {
            path
        };
        normalize_path(path)
    };

    let has_packaged_path =
        |paths: &[PathBuf]| -> bool { paths.iter().any(|path| packaged_files.contains(path)) };

    // Cargo's implicit target path rules when `path` is not set:
    // - lib: src/lib.rs
    // - bin: src/bin/<name>.rs or src/bin/<name>/main.rs (src/main.rs only for implicit default bin)
    // - example/test/bench: <dir>/<name>.rs or <dir>/<name>/main.rs
    let candidate_paths_for_target =
        |kind: &str, name: Option<&str>, path: Option<&str>, package_name: Option<&str>| {
            if let Some(path) = path {
                return vec![normalize(Path::new(path))];
            }

            let name = name.or(package_name);
            match (kind, name) {
                ("lib", _) => vec![normalize(Path::new("src/lib.rs"))],
                ("bin", Some(name)) => {
                    vec![
                        normalize(Path::new(&format!("src/bin/{name}.rs"))),
                        normalize(Path::new(&format!("src/bin/{name}/main.rs"))),
                    ]
                }
                ("bin", None) => vec![normalize(Path::new("src/main.rs"))],
                ("example", Some(name)) => vec![
                    normalize(Path::new(&format!("examples/{name}.rs"))),
                    normalize(Path::new(&format!("examples/{name}/main.rs"))),
                ],
                ("test", Some(name)) => vec![
                    normalize(Path::new(&format!("tests/{name}.rs"))),
                    normalize(Path::new(&format!("tests/{name}/main.rs"))),
                ],
                ("bench", Some(name)) => vec![
                    normalize(Path::new(&format!("benches/{name}.rs"))),
                    normalize(Path::new(&format!("benches/{name}/main.rs"))),
                ],
                _ => Vec::new(),
            }
        };

    let package_name = package_name.as_deref();

    let mut drop_lib = false;
    if let Some(lib) = document.get("lib").and_then(|item| item.as_table()) {
        let name = lib.get("name").and_then(|item| item.as_str());
        let path = lib.get("path").and_then(|item| item.as_str());
        let candidates = candidate_paths_for_target("lib", name, path, package_name);
        if !candidates.is_empty() && !has_packaged_path(&candidates) {
            debug!(
                "Stripping [lib] target {:?} from {}",
                name.or(path),
                manifest_path.display()
            );
            drop_lib = true;
        }
    }

    if drop_lib {
        document.remove("lib");
    }

    let mut removed_bins = Vec::new();
    for (key, kind) in [
        ("bin", "bin"),
        ("example", "example"),
        ("test", "test"),
        ("bench", "bench"),
    ] {
        if let Some(targets) = document
            .get_mut(key)
            .and_then(|item| item.as_array_of_tables_mut())
        {
            let mut idx = 0;
            while idx < targets.len() {
                let target = targets.get(idx).unwrap();
                let name = target.get("name").and_then(|item| item.as_str());
                let path = target.get("path").and_then(|item| item.as_str());
                let candidates = candidate_paths_for_target(kind, name, path, package_name);
                if !candidates.is_empty() && !has_packaged_path(&candidates) {
                    debug!(
                        "Stripping {key} target {:?} from {}",
                        name.or(path),
                        manifest_path.display()
                    );
                    if kind == "bin"
                        && let Some(name) = name
                    {
                        removed_bins.push(name.to_string());
                    }
                    targets.remove(idx);
                } else {
                    idx += 1;
                }
            }
            if targets.is_empty() {
                document.remove(key);
            }
        }
    }

    // If we removed any binaries, we must check if they were the `default-run` target.
    // If so, we remove `default-run` to prevent `cargo run` from failing with a missing target.
    if !removed_bins.is_empty()
        && let Some(package) = document
            .get_mut("package")
            .and_then(|item| item.as_table_mut())
        && let Some(default_run) = package.get("default-run").and_then(|item| item.as_str())
        && removed_bins.iter().any(|name| name == default_run)
    {
        debug!(
            "Stripping [package.default-run] target {:?} from {}",
            default_run,
            manifest_path.display()
        );
        package.remove("default-run");
    }

    Ok(())
}

/// Rewrite a `[package]` field in `Cargo.toml` to point to a file in the same directory.
///
/// Fields like `readme` and `license-file` may reference files above the package.
/// When we flatten the directory structure in the sdist, the path needs updating.
pub(super) fn rewrite_cargo_toml_package_field(
    document: &mut DocumentMut,
    manifest_path: &Path,
    field: &str,
    value: Option<&str>,
) -> Result<()> {
    if let Some(value) = value {
        debug!(
            "Rewriting Cargo.toml `package.{field}` at {}",
            manifest_path.display()
        );
        let project = document.get_mut("package").with_context(|| {
            format!(
                "Missing `[package]` table in Cargo.toml with {field} at {}",
                manifest_path.display()
            )
        })?;
        project[field] = toml_edit::value(value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolved_dep_to_toml_path_dep() {
        let dep = ResolvedDep {
            req: "*".to_string(),
            optional: false,
            default_features: true,
            features: vec![],
            package: None,
            path: Some(PathBuf::from("../shared_crate")),
        };
        let item = resolved_dep_to_toml(&dep);
        let s = item.to_string();
        assert!(
            s.contains(r#"path = "../shared_crate""#),
            "expected path dep, got: {s}"
        );
        assert!(
            !s.contains("version"),
            "should not have version for path dep, got: {s}"
        );
    }

    #[test]
    fn test_resolved_dep_to_toml_renamed() {
        let dep = ResolvedDep {
            req: "1.0".to_string(),
            optional: false,
            default_features: true,
            features: vec![],
            package: Some("real_crate".to_string()),
            path: None,
        };
        let item = resolved_dep_to_toml(&dep);
        let s = item.to_string();
        assert!(
            s.contains(r#"package = "real_crate""#),
            "expected package = real_crate, got: {s}"
        );
    }

    #[test]
    fn test_rewrite_cargo_toml_package_field() {
        let manifest_path = Path::new("Cargo.toml");

        // When value is Some, it should rewrite the field
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"
license-file = "../../LICENSE"
"#;
        let mut document = toml_str.parse::<DocumentMut>().unwrap();
        rewrite_cargo_toml_package_field(
            &mut document,
            manifest_path,
            "license-file",
            Some("LICENSE"),
        )
        .unwrap();
        let result = document.to_string();
        assert!(
            result.contains(r#"license-file = "LICENSE""#),
            "expected rewritten license-file, got: {result}"
        );

        // When value is None, it should be a no-op
        let mut document2 = toml_str.parse::<DocumentMut>().unwrap();
        rewrite_cargo_toml_package_field(&mut document2, manifest_path, "license-file", None)
            .unwrap();
        let result2 = document2.to_string();
        assert!(
            result2.contains(r#"license-file = "../../LICENSE""#),
            "expected unchanged license-file, got: {result2}"
        );
    }

    #[test]
    fn test_rewrite_cargo_toml_removes_default_members() {
        let manifest_path = Path::new("Cargo.toml");
        let toml_str = r#"
[workspace]
members = ["crate-a", "crate-b"]
default-members = ["crate-a", "crate-c"]
"#;
        let mut document = toml_str.parse::<DocumentMut>().unwrap();
        let mut known_path_deps = HashMap::new();
        known_path_deps.insert(
            "crate-a".to_string(),
            PathDependency {
                manifest_path: PathBuf::from("crate-a/Cargo.toml"),
                workspace_root: PathBuf::from(""),
                readme: None,
                license_file: None,
                resolved_package: None,
            },
        );
        rewrite_cargo_toml(&mut document, manifest_path, &known_path_deps).unwrap();
        let result = document.to_string();
        assert!(
            result.contains(r#"members = ["crate-a"]"#),
            "expected filtered members, got: {result}"
        );
        assert!(
            !result.contains("default-members"),
            "expected default-members to be removed, got: {result}"
        );
    }
}
