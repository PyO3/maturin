use super::PathDependency;
use super::utils::{normalize_path, relative_path};
use anyhow::{Context, Result, bail};
use cargo_metadata::DependencyKind;
use fs_err as fs;
use path_slash::PathExt as _;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Table};
use tracing::debug;
use url::Url;

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
/// sibling workspace members). Without this, `field.workspace = true` entries
/// would fail to resolve when building from the sdist.
pub(super) fn resolve_workspace_inheritance(
    document: &mut DocumentMut,
    resolved: &cargo_metadata::Package,
    workspace_inheritance: Option<&WorkspaceManifestInheritance>,
) -> Result<()> {
    resolve_workspace_package_fields(
        document,
        resolved,
        workspace_inheritance.and_then(|inheritance| inheritance.package.as_ref()),
    )?;
    resolve_workspace_dependency_tables(
        document,
        resolved,
        workspace_inheritance.and_then(|inheritance| inheritance.dependencies.as_ref()),
    );
    resolve_workspace_target_dependency_tables(
        document,
        resolved,
        workspace_inheritance.and_then(|inheritance| inheritance.dependencies.as_ref()),
    );
    resolve_workspace_lints(
        document,
        workspace_inheritance.and_then(|inheritance| inheritance.lints.as_ref()),
    )?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub(super) struct WorkspaceManifestInheritance {
    pub(super) package: Option<Table>,
    pub(super) dependencies: Option<Table>,
    pub(super) lints: Option<toml_edit::Item>,
}

pub(super) fn parse_workspace_manifest_inheritance(
    workspace_manifest_path: &Path,
) -> Result<WorkspaceManifestInheritance> {
    let document = parse_toml_file(workspace_manifest_path, "workspace Cargo.toml")?;
    let workspace = document.get("workspace").and_then(|item| item.as_table());
    Ok(WorkspaceManifestInheritance {
        package: workspace
            .and_then(|workspace| workspace.get("package"))
            .and_then(|item| item.as_table())
            .cloned(),
        dependencies: workspace
            .and_then(|workspace| workspace.get("dependencies"))
            .and_then(|item| item.as_table())
            .cloned(),
        lints: workspace
            .and_then(|workspace| workspace.get("lints"))
            .cloned(),
    })
}

fn resolve_workspace_package_fields(
    document: &mut DocumentMut,
    resolved: &cargo_metadata::Package,
    workspace_package: Option<&Table>,
) -> Result<()> {
    let Some(package) = document.get_mut("package").and_then(|p| p.as_table_mut()) else {
        return Ok(());
    };

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

    // `cargo metadata` already resolves `publish`, unlike `include`/`exclude`.
    if is_workspace_inherited(package, "publish") {
        match resolved.publish.as_ref() {
            None => {
                package.remove("publish");
            }
            Some(registries) if registries.is_empty() => {
                package.insert("publish", toml_edit::value(false));
            }
            Some(registries) => {
                let mut arr = toml_edit::Array::new();
                for registry in registries {
                    arr.push(registry.as_str());
                }
                package.insert("publish", toml_edit::value(arr));
            }
        }
    }

    for key in ["include", "exclude"] {
        inherit_workspace_package_item(package, key, workspace_package)?;
    }

    for key in ["readme", "license-file"] {
        if is_workspace_inherited(package, key) {
            package.remove(key);
        }
    }

    Ok(())
}

fn resolve_workspace_dependency_tables(
    document: &mut DocumentMut,
    resolved: &cargo_metadata::Package,
    workspace_dependencies: Option<&Table>,
) {
    for dep_kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
        resolve_workspace_deps(document, dep_kind, resolved, workspace_dependencies);
    }
}

fn resolve_workspace_target_dependency_tables(
    document: &mut DocumentMut,
    resolved: &cargo_metadata::Package,
    workspace_dependencies: Option<&Table>,
) {
    let Some(target) = document.get_mut("target").and_then(|t| t.as_table_mut()) else {
        return;
    };

    let target_keys: Vec<String> = target.iter().map(|(k, _)| k.to_string()).collect();
    for target_key in target_keys {
        let Some(target_val) = target.get_mut(&target_key) else {
            continue;
        };
        for dep_kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
            let Some(deps) = target_val.get_mut(dep_kind).and_then(|t| t.as_table_mut()) else {
                continue;
            };
            resolve_workspace_dep_entries(
                deps,
                resolved,
                workspace_dependencies,
                Some(&target_key),
                dep_kind_from_str(dep_kind),
                &format!("target.{target_key}.{dep_kind}"),
            );
        }
    }
}

fn resolve_workspace_lints(
    document: &mut DocumentMut,
    workspace_lints: Option<&toml_edit::Item>,
) -> Result<()> {
    let inherits_workspace_lints = document
        .get("lints")
        .and_then(|item| item.as_table_like())
        .and_then(|table| table.get("workspace"))
        .and_then(|value| value.as_bool())
        == Some(true);
    if !inherits_workspace_lints {
        return Ok(());
    }

    let Some(item) = workspace_lints else {
        bail!("Failed to resolve workspace-inherited `lints`");
    };
    document.as_table_mut().insert("lints", item.clone());
    Ok(())
}

fn inherit_workspace_package_item(
    package: &mut Table,
    key: &str,
    workspace_package: Option<&Table>,
) -> Result<()> {
    if !is_workspace_inherited(package, key) {
        return Ok(());
    }

    let Some(item) = workspace_package.and_then(|workspace_package| workspace_package.get(key))
    else {
        bail!("Failed to resolve workspace-inherited `package.{key}`");
    };
    package.insert(key, item.clone());
    Ok(())
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
/// Resolve workspace-inherited dependencies in a single dependency table.
///
/// This is the shared inner loop used by both top-level dependency tables
/// (e.g. `[dependencies]`) and target-specific ones (e.g. `[target.*.dependencies]`).
fn resolve_workspace_dep_entries(
    deps: &mut Table,
    resolved: &cargo_metadata::Package,
    workspace_dependencies: Option<&Table>,
    target: Option<&str>,
    dep_kind: DependencyKind,
    label: &str,
) {
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
        if let Some(resolved_dep) = find_resolved_dep(resolved, &dep_name, target, dep_kind) {
            deps.insert(
                &dep_name,
                resolved_dep_to_toml(
                    &resolved_dep,
                    workspace_dependencies.and_then(|d| d.get(&dep_name)),
                ),
            );
        } else {
            debug!("Could not resolve workspace dependency {dep_name} in {label}");
        }
    }
}

fn resolve_workspace_deps(
    document: &mut DocumentMut,
    dep_kind: &str,
    resolved: &cargo_metadata::Package,
    workspace_dependencies: Option<&Table>,
) {
    let Some(deps_table) = document.get_mut(dep_kind).and_then(|t| t.as_table_mut()) else {
        return;
    };
    resolve_workspace_dep_entries(
        deps_table,
        resolved,
        workspace_dependencies,
        None,
        dep_kind_from_str(dep_kind),
        &format!("[{dep_kind}]"),
    );
}

/// Maps a Cargo.toml dependency table name to the corresponding `DependencyKind`.
fn dep_kind_from_str(dep_kind: &str) -> DependencyKind {
    match dep_kind {
        "dev-dependencies" => DependencyKind::Development,
        "build-dependencies" => DependencyKind::Build,
        _ => DependencyKind::Normal,
    }
}

#[derive(Debug, Clone)]
struct GitSource {
    url: String,
    branch: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
}

#[derive(Debug, Clone)]
enum DepSource {
    CratesIo,
    Path(PathBuf),
    Git(GitSource),
    Registry(String),
}

/// Finds a resolved dependency by name in the package metadata.
fn find_resolved_dep(
    resolved: &cargo_metadata::Package,
    name: &str,
    target: Option<&str>,
    kind: DependencyKind,
) -> Option<ResolvedDep> {
    resolved.dependencies.iter().find_map(|d| {
        let matches_name = d.rename.as_deref() == Some(name) || d.name == name;
        if !matches_name {
            return None;
        }
        if d.kind != kind {
            return None;
        }
        if let Some(target_str) = target
            && d.target.as_ref().map(|t| t.to_string()).as_deref() != Some(target_str)
        {
            return None;
        }

        let package = d.rename.as_ref().map(|_| d.name.clone());
        let source = if let Some(dep_path) = d.path.as_ref() {
            let manifest_dir = resolved.manifest_path.parent().unwrap();
            DepSource::Path(relative_path(
                manifest_dir.as_std_path(),
                dep_path.as_std_path(),
            ))
        } else if let Some(git) = d.source.as_ref().and_then(parse_git_source) {
            DepSource::Git(git)
        } else if let Some(registry) = &d.registry {
            DepSource::Registry(registry.clone())
        } else {
            DepSource::CratesIo
        };

        Some(ResolvedDep {
            req: d.req.to_string(),
            optional: d.optional,
            default_features: d.uses_default_features,
            features: d.features.clone(),
            package,
            source,
        })
    })
}

fn parse_git_source(source: &cargo_metadata::Source) -> Option<GitSource> {
    let source = source.repr.strip_prefix("git+")?;
    let mut url = Url::parse(source).ok()?;

    let mut git = GitSource {
        url: String::new(),
        branch: None,
        tag: None,
        rev: None,
    };
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "branch" => git.branch = Some(value.into_owned()),
            "tag" => git.tag = Some(value.into_owned()),
            "rev" => git.rev = Some(value.into_owned()),
            _ => {}
        }
    }
    if git.branch.is_none()
        && git.tag.is_none()
        && git.rev.is_none()
        && let Some(fragment) = url.fragment()
        && !fragment.is_empty()
    {
        git.rev = Some(fragment.to_string());
    }
    url.set_query(None);
    url.set_fragment(None);
    git.url = url.into();
    Some(git)
}

struct ResolvedDep {
    req: String,
    optional: bool,
    default_features: bool,
    features: Vec<String>,
    /// When the dep is renamed (aliased), the real package name.
    /// Emitted as `package = "<name>"` in Cargo.toml.
    package: Option<String>,
    source: DepSource,
}

fn merge_registry_workspace_dependency(
    original: &toml_edit::Item,
    dep: &ResolvedDep,
) -> toml_edit::Item {
    let mut item = original.clone();
    let Some(table) = item.as_table_like_mut() else {
        // Cargo manifests use `registry = "<name>"`, but cargo metadata only
        // exposes the registry index URL. If we cannot recover the original
        // table form from `[workspace.dependencies]`, fall back to a plain
        // version requirement instead of emitting the invalid
        // `registry-index = ...` key.
        return toml_edit::value(dep.req.as_str());
    };

    table.insert("version", toml_edit::value(dep.req.as_str()));
    if dep.optional {
        table.insert("optional", toml_edit::value(true));
    } else {
        table.remove("optional");
    }
    if !dep.default_features {
        table.insert("default-features", toml_edit::value(false));
    }
    if !dep.features.is_empty() {
        let mut arr = toml_edit::Array::new();
        for f in &dep.features {
            arr.push(f.as_str());
        }
        table.insert("features", toml_edit::value(arr));
    } else {
        table.remove("features");
    }
    if let Some(package) = &dep.package {
        table.insert("package", toml_edit::value(package.as_str()));
    }

    item
}

/// Converts a resolved dependency into a TOML value for Cargo.toml.
fn resolved_dep_to_toml(
    dep: &ResolvedDep,
    workspace_dependency: Option<&toml_edit::Item>,
) -> toml_edit::Item {
    if matches!(dep.source, DepSource::CratesIo)
        && dep.default_features
        && !dep.optional
        && dep.features.is_empty()
        && dep.package.is_none()
    {
        return toml_edit::value(&dep.req);
    }

    let mut table = toml_edit::InlineTable::new();
    match &dep.source {
        DepSource::CratesIo => {
            table.insert("version", dep.req.as_str().into());
        }
        DepSource::Path(path) => {
            let path_str = path.to_slash().unwrap_or_else(|| path.to_string_lossy());
            table.insert("path", path_str.as_ref().into());
            if dep.req != "*" {
                table.insert("version", dep.req.as_str().into());
            }
        }
        DepSource::Git(git) => {
            table.insert("git", git.url.as_str().into());
            if let Some(branch) = &git.branch {
                table.insert("branch", branch.as_str().into());
            }
            if let Some(tag) = &git.tag {
                table.insert("tag", tag.as_str().into());
            }
            if let Some(rev) = &git.rev {
                table.insert("rev", rev.as_str().into());
            }
            if dep.req != "*" {
                table.insert("version", dep.req.as_str().into());
            }
        }
        DepSource::Registry(_registry_index) => {
            return workspace_dependency
                .map(|item| merge_registry_workspace_dependency(item, dep))
                .unwrap_or_else(|| toml_edit::value(dep.req.as_str()));
        }
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
//
// We rely on `cargo metadata` for target discovery and source paths, and only
// use the packaged file set to determine which of those discovered targets
// still exist in the sdist.
pub(super) fn rewrite_cargo_toml_targets(
    document: &mut DocumentMut,
    manifest_path: &Path,
    package_metadata: &cargo_metadata::Package,
    packaged_files: &HashSet<PathBuf>,
) -> Result<()> {
    debug!(
        "Rewriting Cargo.toml build targets at {}",
        manifest_path.display()
    );

    let manifest_dir = manifest_path.parent().unwrap();

    let normalize = |path: &Path| -> PathBuf {
        let path = if path.is_absolute() {
            path.strip_prefix(manifest_dir).unwrap_or(path)
        } else {
            path
        };
        normalize_path(path)
    };

    let target_src_path =
        |target: &cargo_metadata::Target| normalize(target.src_path.as_std_path());
    let is_packaged_target =
        |target: &cargo_metadata::Target| packaged_files.contains(&target_src_path(target));

    let matches_kind = |target: &cargo_metadata::Target, kind: &str| match kind {
        "lib" => target.kind.iter().any(|kind| {
            matches!(
                kind,
                cargo_metadata::TargetKind::Lib
                    | cargo_metadata::TargetKind::RLib
                    | cargo_metadata::TargetKind::DyLib
                    | cargo_metadata::TargetKind::CDyLib
                    | cargo_metadata::TargetKind::StaticLib
                    | cargo_metadata::TargetKind::ProcMacro
            )
        }),
        "bin" => target.is_bin(),
        "example" => target.is_example(),
        "test" => target.is_test(),
        "bench" => target.is_bench(),
        _ => false,
    };

    let find_matching_target = |kind: &str, name: Option<&str>, path: Option<&str>| {
        let normalized_path = path.map(|path| normalize(Path::new(path)));
        package_metadata.targets.iter().find(|target| {
            if !matches_kind(target, kind) {
                return false;
            }
            if let Some(path) = normalized_path.as_ref()
                && &target_src_path(target) != path
            {
                return false;
            }
            if kind == "lib" {
                return name.is_none_or(|name| target.name == name);
            }
            name.is_some_and(|name| target.name == name)
        })
    };

    if let Some(lib) = document.get("lib").and_then(|item| item.as_table()) {
        let name = lib.get("name").and_then(|item| item.as_str());
        let path = lib.get("path").and_then(|item| item.as_str());
        if let Some(target) = find_matching_target("lib", name, path)
            && !is_packaged_target(target)
        {
            debug!(
                "Stripping [lib] target {:?} from {}",
                name.or(path),
                manifest_path.display()
            );
            document.remove("lib");
        }
    }

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
                let should_remove = find_matching_target(kind, name, path)
                    .is_some_and(|target| !is_packaged_target(target));
                if should_remove {
                    debug!(
                        "Stripping {key} target {:?} from {}",
                        name.or(path),
                        manifest_path.display()
                    );
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

    let packaged_bin_names: HashSet<&str> = package_metadata
        .targets
        .iter()
        .filter(|target| target.is_bin() && is_packaged_target(target))
        .map(|target| target.name.as_str())
        .collect();

    if let Some(package) = document
        .get_mut("package")
        .and_then(|item| item.as_table_mut())
    {
        // Remove `build` when the explicitly-configured build script source was
        // excluded from the sdist. Cargo treats explicit `package.build = ...`
        // differently from implicit `build.rs`: if the path is configured but the
        // file is missing, manifest loading fails.
        if let Some(build_path) = package.get("build").and_then(|item| item.as_str()) {
            let normalized_build_path = normalize(Path::new(build_path));
            let explicit_build_missing = package_metadata.targets.iter().any(|target| {
                target.is_custom_build()
                    && target_src_path(target) == normalized_build_path
                    && !is_packaged_target(target)
            });
            if explicit_build_missing {
                debug!(
                    "Stripping [package.build] target {:?} from {}",
                    build_path,
                    manifest_path.display()
                );
                package.remove("build");
            }
        }

        // Remove `default-run` when its target was excluded from the sdist. This
        // uses cargo metadata rather than hand-rolled path inference, so it also
        // handles implicit default bins like `src/main.rs`.
        if let Some(default_run) = package.get("default-run").and_then(|item| item.as_str())
            && package_metadata
                .targets
                .iter()
                .any(|target| target.is_bin() && target.name == default_run)
            && !packaged_bin_names.contains(default_run)
        {
            debug!(
                "Stripping [package.default-run] target {:?} from {}",
                default_run,
                manifest_path.display()
            );
            package.remove("default-run");
        }
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
            source: DepSource::Path(PathBuf::from("../shared_crate")),
        };
        let item = resolved_dep_to_toml(&dep, None);
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
            source: DepSource::CratesIo,
        };
        let item = resolved_dep_to_toml(&dep, None);
        let s = item.to_string();
        assert!(
            s.contains(r#"package = "real_crate""#),
            "expected package = real_crate, got: {s}"
        );
    }

    #[test]
    fn test_resolved_dep_to_toml_git_dep() {
        let dep = ResolvedDep {
            req: "*".to_string(),
            optional: false,
            default_features: true,
            features: vec![],
            package: None,
            source: DepSource::Git(GitSource {
                url: "https://example.com/repo.git".to_string(),
                branch: Some("main".to_string()),
                tag: None,
                rev: None,
            }),
        };
        let item = resolved_dep_to_toml(&dep, None);
        let s = item.to_string();
        assert!(
            s.contains(r#"git = "https://example.com/repo.git""#),
            "expected git source, got: {s}"
        );
        assert!(
            s.contains(r#"branch = "main""#),
            "expected git branch, got: {s}"
        );
        assert!(
            !s.contains("version"),
            "should not add version = \"*\" for plain git dep, got: {s}"
        );
    }

    #[test]
    fn test_resolved_dep_to_toml_registry_dep_falls_back_to_version_only() {
        let dep = ResolvedDep {
            req: "1.0".to_string(),
            optional: false,
            default_features: true,
            features: vec![],
            package: None,
            source: DepSource::Registry("https://example.com/index".to_string()),
        };
        let item = resolved_dep_to_toml(&dep, None);
        assert_eq!(item.to_string(), "\"1.0\"");
    }

    #[test]
    fn test_resolved_dep_to_toml_registry_dep_preserves_registry_name() {
        let dep = ResolvedDep {
            req: "1.5".to_string(),
            optional: true,
            default_features: true,
            features: vec!["unicode".to_string()],
            package: None,
            source: DepSource::Registry("https://example.com/index".to_string()),
        };
        let workspace_dep: toml_edit::Item =
            r#"{ version = "1.0", registry = "custom", features = ["std"] }"#
                .parse::<toml_edit::Value>()
                .unwrap()
                .into();
        let item = resolved_dep_to_toml(&dep, Some(&workspace_dep));
        let s = item.to_string();
        assert!(
            s.contains(r#"registry = "custom""#),
            "expected registry name, got: {s}"
        );
        assert!(
            s.contains(r#"version = "1.5""#),
            "expected version, got: {s}"
        );
        assert!(
            s.contains(r#"optional = true"#),
            "expected optional, got: {s}"
        );
        assert!(
            s.contains(r#"features = ["unicode"]"#),
            "expected merged features, got: {s}"
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
