use crate::module_writer::{add_data, ModuleWriter};
use crate::{pyproject_toml::Format, BuildContext, PyProjectToml, SDistWriter};
use anyhow::{bail, Context, Result};
use cargo_metadata::{Metadata, MetadataCommand};
use fs_err as fs;
use ignore::overrides::Override;
use normpath::PathExt as _;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str;
use tracing::debug;

const LOCAL_DEPENDENCIES_FOLDER: &str = "local_dependencies";
const RUST_SRC_FOLDER: &str = "rust_src";

/// Inheritable workspace fields, see
/// https://github.com/rust-lang/cargo/blob/13ae438cf079da58272edc71f4d4968043dbd27b/src/cargo/util/toml/mod.rs#L1140-L1158
const WORKSPACE_INHERITABLE_FIELDS: &[&str] = &[
    "version",
    "authors",
    "description",
    "homepage",
    "documentation",
    "readme",
    "keywords",
    "categories",
    "license",
    "license-file",
    "repository",
    "publish",
    "edition",
    "badges",
    "exclude",
    "include",
    "rust-version",
];

/// We need cargo to load the local dependencies from the location where we put them in the source
/// distribution. Since there is no cargo-backed way to replace dependencies
/// (see https://github.com/rust-lang/cargo/issues/9170), we do a simple
/// Cargo.toml rewrite ourselves.
/// A big chunk of that comes from cargo edit, and esp.
/// https://github.com/killercup/cargo-edit/blob/2a08f0311bcb61690d71d39cb9e55e69b256c8e1/src/manifest.rs
/// This method is rather frail, but unfortunately I don't know a better solution.
fn rewrite_cargo_toml(
    manifest_path: impl AsRef<Path>,
    workspace_manifest: &toml_edit::Document,
    known_path_deps: &HashMap<String, PathBuf>,
    local_deps_folder: String,
    root_crate: bool,
) -> Result<String> {
    let manifest_path = manifest_path.as_ref();
    let text = fs::read_to_string(manifest_path).context(format!(
        "Can't read Cargo.toml at {}",
        manifest_path.display(),
    ))?;
    let mut data = text.parse::<toml_edit::Document>().context(format!(
        "Failed to parse Cargo.toml at {}",
        manifest_path.display()
    ))?;
    let mut rewritten = false;
    //  ˇˇˇˇˇˇˇˇˇˇˇˇ dep_category
    // [dependencies]
    // some_path_dep = { path = "../some_path_dep" }
    //                          ^^^^^^^^^^^^^^^^^^ table[&dep_name]["path"]
    // ^^^^^^^^^^^^^ dep_name
    for dep_category in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = data.get_mut(dep_category).and_then(|x| x.as_table_mut()) {
            if dep_category == "dev-dependencies" && !known_path_deps.is_empty() {
                // Remove dev-dependencies since building from sdist doesn't need them,
                // Keep it when there are no path dependencies to support building from
                // sdist with `--locked`/`--frozen`.
                data.remove(dep_category);
                rewritten = true;
                continue;
            }
            let workspace_deps = workspace_manifest
                .get("workspace")
                .and_then(|x| x.get("dependencies"))
                .and_then(|x| x.as_table_like());
            let dep_names: Vec<_> = table.iter().map(|(key, _)| key.to_string()).collect();
            for dep_name in dep_names {
                let workspace_inherit = table
                    .get(&dep_name)
                    .and_then(|x| x.get("workspace"))
                    .and_then(|x| x.as_bool())
                    .unwrap_or_default();

                if !workspace_inherit {
                    // There should either be no value for path, or it should be a string
                    if table.get(&dep_name).and_then(|x| x.get("path")).is_none() {
                        continue;
                    }
                    if !table[&dep_name]["path"].is_str() {
                        bail!(
                            "In {}, {} {} has a path value that is not a string",
                            manifest_path.display(),
                            dep_category,
                            dep_name
                        )
                    }
                    if !known_path_deps.contains_key(&dep_name) {
                        bail!(
                            "cargo metadata does not know about the path for {}.{} present in {}, \
                            which should never happen ಠ_ಠ",
                            dep_category,
                            dep_name,
                            manifest_path.display()
                        );
                    }
                } else {
                    // If a workspace inherited dependency isn't a path dep,
                    // we need to replace `workspace = true` with its full requirement spec.
                    if !known_path_deps.contains_key(&dep_name) {
                        if let Some(workspace_dep) = workspace_deps.and_then(|x| x.get(&dep_name)) {
                            let mut workspace_dep = workspace_dep.clone();
                            // Merge optional and features from the current Cargo.toml
                            if table[&dep_name].get("optional").is_some() {
                                ensure_dep_is_inline_table(&mut workspace_dep);
                                workspace_dep["optional"] = table[&dep_name]["optional"].clone();
                            }
                            if let Some(features) =
                                table[&dep_name].get("features").and_then(|x| x.as_array())
                            {
                                ensure_dep_is_inline_table(&mut workspace_dep);
                                let existing_features = workspace_dep
                                    .as_table_like_mut()
                                    .unwrap()
                                    .entry("features")
                                    .or_insert_with(|| {
                                        toml_edit::Item::Value(toml_edit::Array::new().into())
                                    })
                                    .as_array_mut()
                                    .with_context(|| {
                                        format!(
                                            "In {}, {} {} has a features value that is not an array",
                                            manifest_path.display(),
                                            dep_category,
                                            dep_name
                                        )
                                    })?;
                                existing_features.extend(features);
                            }
                            table[&dep_name] = workspace_dep;
                            rewritten = true;
                        } else {
                            bail!(
                                "In {}, {} {} is marked as `workspace = true`, but it is found neither in \
                                the workspace manifest nor in the known path dependencies",
                                manifest_path.display(),
                                dep_category,
                                dep_name
                            )
                        }
                        continue;
                    }
                }
                // This is the location of the targeted crate in the source distribution
                table[&dep_name]["path"] = if root_crate {
                    toml_edit::value(format!("{local_deps_folder}/{dep_name}"))
                } else {
                    // Cargo.toml contains relative paths, and we're already in LOCAL_DEPENDENCIES_FOLDER
                    toml_edit::value(format!("../{dep_name}"))
                };
                if workspace_inherit {
                    // Remove workspace inheritance now that we converted it into a path dependency
                    table[&dep_name]
                        .as_table_like_mut()
                        .unwrap()
                        .remove("workspace");
                }
                rewritten = true;
            }
        }
    }

    // Update workspace inherited metadata
    if let Some(package) = data.get_mut("package").and_then(|x| x.as_table_mut()) {
        let workspace_package = workspace_manifest
            .get("workspace")
            .and_then(|x| x.get("package"))
            .and_then(|x| x.as_table_like());
        for key in WORKSPACE_INHERITABLE_FIELDS.iter().copied() {
            let workspace_inherited = package
                .get(key)
                .and_then(|x| x.get("workspace"))
                .and_then(|x| x.as_bool())
                .unwrap_or_default();
            if workspace_inherited {
                if let Some(workspace_value) = workspace_package.and_then(|ws| ws.get(key)) {
                    package[key] = workspace_value.clone();
                    rewritten = true;
                }
            }
        }
    }

    if root_crate {
        // Update workspace members
        if let Some(workspace) = data.get_mut("workspace").and_then(|x| x.as_table_mut()) {
            if let Some(members) = workspace.get_mut("members").and_then(|x| x.as_array_mut()) {
                if known_path_deps.is_empty() {
                    // Remove workspace members when there isn't any path dep
                    workspace.remove("members");
                    if workspace.is_empty() {
                        // Remove workspace all together if it's empty
                        data.remove("workspace");
                    }
                    rewritten = true;
                } else {
                    let mut new_members = toml_edit::Array::new();
                    for member in members.iter() {
                        if let toml_edit::Value::String(ref s) = member {
                            let path = Path::new(s.value());
                            if let Some(name) = path.file_name().and_then(|x| x.to_str()) {
                                if known_path_deps.contains_key(name) {
                                    new_members.push(format!("{LOCAL_DEPENDENCIES_FOLDER}/{name}"));
                                }
                            }
                        }
                    }
                    if !new_members.is_empty() {
                        workspace["members"] = toml_edit::value(new_members);
                    } else {
                        workspace.remove("members");
                    }
                    rewritten = true;
                }
            }
        }
    } else {
        // Update package.workspace
        // https://rust-lang.github.io/rfcs/1525-cargo-workspace.html#implicit-relations
        // https://doc.rust-lang.org/cargo/reference/manifest.html#the-workspace-field
        if let Some(package) = data.get_mut("package").and_then(|x| x.as_table_mut()) {
            if let Some(workspace) = package.get("workspace").and_then(|x| x.as_str()) {
                // This is enough to fix https://github.com/PyO3/maturin/issues/838
                // Other cases can be fixed on demand
                if workspace == ".." || workspace == "../" {
                    package.remove("workspace");
                    rewritten = true;
                }
            }
        }
    }
    if rewritten {
        Ok(data.to_string())
    } else {
        Ok(text)
    }
}

/// Make sure that the dep entry is an inline table
/// e.g. in the form of `{ version = "..." }`
/// so that we can add entries for `optional` and `features`
fn ensure_dep_is_inline_table(dep: &mut toml_edit::Item) {
    if let Some(v) = dep.as_value_mut() {
        if v.is_str() {
            let val = std::mem::replace(v, toml_edit::Value::from(false));
            let mut tab = toml_edit::InlineTable::new();
            tab.insert("version", val);
            *v = toml_edit::Value::InlineTable(tab);
        }
    }
}

/// When `Cargo.toml` is outside of the directory containing `pyproject.toml`,
/// we put Rust crate source to `RUST_SRC_FOLDER` and
/// update `tool.maturin.manifest-path` in `pyproject.toml`.
fn rewrite_pyproject_toml(pyproject_toml_path: &Path) -> Result<String> {
    let text = fs::read_to_string(pyproject_toml_path).context(format!(
        "Can't read pyproject.toml at {}",
        pyproject_toml_path.display(),
    ))?;
    let mut data = text.parse::<toml_edit::Document>().context(format!(
        "Failed to parse pyproject.toml at {}",
        pyproject_toml_path.display()
    ))?;
    if let Some(tool) = data.get_mut("tool").and_then(|x| x.as_table_mut()) {
        if let Some(maturin) = tool.get_mut("maturin").and_then(|x| x.as_table_mut()) {
            if let Some(manifest_path) = maturin.get_mut("manifest-path") {
                // original: ../$crate/Cargo.toml or ../../$crate/Cargo.toml
                // rewrite to: $RUST_SRC_FOLDER/$crate/Cargo.toml
                let path =
                    Path::new(manifest_path.as_str().context(
                        "tool.maturin.manifest-path in pyproject.toml must be a string",
                    )?);
                let crate_name = path.parent().unwrap().file_name().unwrap();
                let new_path = Path::new(RUST_SRC_FOLDER)
                    .join(crate_name)
                    .join("Cargo.toml");
                *manifest_path = toml_edit::value(new_path.to_str().unwrap());
            }
        }
    }
    Ok(data.to_string())
}

/// Copies the files of a crate to a source distribution, recursively adding path dependencies
/// and rewriting path entries in Cargo.toml
///
/// Runs `cargo package --list --allow-dirty` to obtain a list of files to package.
fn add_crate_to_source_distribution(
    writer: &mut SDistWriter,
    pyproject_toml_path: impl AsRef<Path>,
    manifest_path: impl AsRef<Path>,
    workspace_manifest: &toml_edit::Document,
    prefix: impl AsRef<Path>,
    known_path_deps: &HashMap<String, PathBuf>,
    root_crate: bool,
) -> Result<()> {
    let manifest_path = manifest_path.as_ref();
    let pyproject_toml_path = pyproject_toml_path.as_ref();
    let output = Command::new("cargo")
        .args(["package", "--list", "--allow-dirty", "--manifest-path"])
        .arg(manifest_path)
        .output()
        .with_context(|| {
            format!(
                "Failed to run `cargo package --list --allow-dirty --manifest-path {}`",
                manifest_path.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "Failed to query file list from cargo: {}\n--- Manifest path: {}\n--- Stdout:\n{}\n--- Stderr:\n{}",
            output.status,
            manifest_path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let file_list: Vec<&Path> = str::from_utf8(&output.stdout)
        .context("Cargo printed invalid utf-8 ಠ_ಠ")?
        .lines()
        .map(Path::new)
        .collect();

    let abs_manifest_path = manifest_path
        .normalize()
        .with_context(|| format!("failed to normalize path `{}`", manifest_path.display()))?
        .into_path_buf();
    let abs_manifest_dir = abs_manifest_path.parent().unwrap();
    let pyproject_dir = pyproject_toml_path.parent().unwrap();
    let cargo_toml_in_subdir = root_crate
        && abs_manifest_dir != pyproject_dir
        && abs_manifest_dir.starts_with(pyproject_dir);

    // manifest_dir should be a relative path
    let manifest_dir = manifest_path.parent().unwrap();
    let mut target_source: Vec<(PathBuf, PathBuf)> = file_list
        .iter()
        .map(|relative_to_manifests| {
            let relative_to_cwd = manifest_dir.join(relative_to_manifests);
            if root_crate && cargo_toml_in_subdir {
                let relative_to_project_root = abs_manifest_dir
                    .strip_prefix(pyproject_dir)
                    .unwrap()
                    .join(relative_to_manifests);
                (relative_to_project_root, relative_to_cwd)
            } else {
                (relative_to_manifests.to_path_buf(), relative_to_cwd)
            }
        })
        // We rewrite Cargo.toml and add it separately
        .filter(|(target, source)| {
            #[allow(clippy::if_same_then_else)]
            // Skip generated files. See https://github.com/rust-lang/cargo/issues/7938#issuecomment-593280660
            // and https://github.com/PyO3/maturin/issues/449
            if target == Path::new("Cargo.toml.orig") || target == Path::new("Cargo.toml") {
                false
            } else if matches!(target.extension(), Some(ext) if ext == "pyc" || ext == "pyd" || ext == "so") {
                // Technically, `cargo package --list` should handle this,
                // but somehow it doesn't on Alpine Linux running in GitHub Actions,
                // so we do it manually here.
                // See https://github.com/PyO3/maturin/pull/1255#issuecomment-1308838786
                debug!("Ignoring {}", target.display());
                false
            } else {
                source.exists()
            }
        })
        .collect();

    let prefix = prefix.as_ref();
    writer.add_directory(prefix)?;

    let mut cargo_toml_in_rust_src = false;

    if root_crate
        && !target_source
            .iter()
            .any(|(target, _)| target == Path::new("pyproject.toml"))
    {
        // Add pyproject.toml to the source distribution
        if cargo_toml_in_subdir {
            // if Cargo.toml is in subdirectory of pyproject.toml directory
            target_source.push((
                PathBuf::from("pyproject.toml"),
                pyproject_toml_path.to_path_buf(),
            ));
        } else {
            // if pyproject.toml was not included by `cargo package --list`
            // (e.g. because it is in a parent directory)
            // we need to add `cargo package --list` files to a subdirectory
            // and rewrite `tool.maturin.manifest-path` in pyproject.toml
            let crate_name = abs_manifest_dir.file_name().unwrap();
            // check that there isn't already a $RUST_SRC_FOLDER/$crate_name/ folder in python root
            let rust_src = Path::new(RUST_SRC_FOLDER).join(crate_name);
            if target_source
                .iter()
                .any(|(target, _)| target.starts_with(&rust_src))
            {
                bail!(
                    "Cannot add crate {} to source distribution because there is already a {} folder, consider rename it to avoid conflicts",
                    crate_name.to_string_lossy(),
                    rust_src.display(),
                );
            }
            target_source.iter_mut().for_each(|(target, _)| {
                *target = rust_src.join(&target);
            });
            // rewrite `tool.maturin.manifest-path` in pyproject.toml
            let rewritten_pyproject_toml = rewrite_pyproject_toml(pyproject_toml_path)?;
            writer.add_bytes(
                prefix.join("pyproject.toml"),
                rewritten_pyproject_toml.as_bytes(),
            )?;
            cargo_toml_in_rust_src = true;
        }
    }

    let cargo_toml_path = if cargo_toml_in_subdir {
        let relative_manifest_path = abs_manifest_path.strip_prefix(pyproject_dir).unwrap();
        prefix.join(relative_manifest_path)
    } else if cargo_toml_in_rust_src {
        let crate_name = abs_manifest_dir.file_name().unwrap();
        prefix
            .join(RUST_SRC_FOLDER)
            .join(crate_name)
            .join(manifest_path.file_name().unwrap())
    } else {
        prefix.join(manifest_path.file_name().unwrap())
    };

    let local_deps_folder = if cargo_toml_in_subdir {
        let level = abs_manifest_dir
            .strip_prefix(pyproject_dir)
            .unwrap()
            .components()
            .count();
        format!("{}{}", "../".repeat(level), LOCAL_DEPENDENCIES_FOLDER)
    } else if cargo_toml_in_rust_src {
        format!("../../{LOCAL_DEPENDENCIES_FOLDER}")
    } else {
        LOCAL_DEPENDENCIES_FOLDER.to_string()
    };
    let rewritten_cargo_toml = rewrite_cargo_toml(
        manifest_path,
        workspace_manifest,
        known_path_deps,
        local_deps_folder,
        root_crate,
    )?;

    writer.add_bytes(cargo_toml_path, rewritten_cargo_toml.as_bytes())?;

    for (target, source) in target_source {
        writer.add_file(prefix.join(target), source)?;
    }

    Ok(())
}

/// Finds all path dependencies of the crate
fn find_path_deps(cargo_metadata: &Metadata) -> Result<HashMap<String, PathBuf>> {
    let root = cargo_metadata
        .root_package()
        .context("Expected the dependency graph to have a root package")?;
    // scan the dependency graph for path dependencies
    let mut path_deps = HashMap::new();
    let mut stack: Vec<&cargo_metadata::Package> = vec![root];
    while let Some(top) = stack.pop() {
        for dependency in &top.dependencies {
            if let Some(path) = &dependency.path {
                if matches!(dependency.kind, cargo_metadata::DependencyKind::Development) {
                    // Skip dev-only dependency
                    debug!(
                        "Skipping development only dependency {} ({})",
                        dependency.name, path
                    );
                    continue;
                }
                // we search for the respective package by `manifest_path`, there seems
                // to be no way to query the dependency graph given `dependency`
                let dep_manifest_path = path.join("Cargo.toml");
                path_deps.insert(
                    dependency.name.clone(),
                    PathBuf::from(dep_manifest_path.clone()),
                );
                if let Some(dep_package) = cargo_metadata
                    .packages
                    .iter()
                    .find(|package| package.manifest_path == dep_manifest_path)
                {
                    // scan the dependencies of the path dependency
                    stack.push(dep_package)
                }
            }
        }
    }
    Ok(path_deps)
}

/// Creates a source distribution, packing the root crate and all local dependencies
///
/// The source distribution format is specified in
/// [PEP 517 under "build_sdist"](https://www.python.org/dev/peps/pep-0517/#build-sdist)
/// and in
/// https://packaging.python.org/specifications/source-distribution-format/#source-distribution-file-format
pub fn source_distribution(
    build_context: &BuildContext,
    pyproject: &PyProjectToml,
    excludes: Option<Override>,
) -> Result<PathBuf> {
    let metadata21 = &build_context.metadata21;
    let manifest_path = &build_context.manifest_path;
    let pyproject_toml_path = build_context
        .pyproject_toml_path
        .normalize()
        .with_context(|| {
            format!(
                "failed to normalize path `{}`",
                build_context.pyproject_toml_path.display()
            )
        })?
        .into_path_buf();
    let workspace_manifest_path = build_context
        .cargo_metadata
        .workspace_root
        .join("Cargo.toml");
    let workspace_manifest: toml_edit::Document =
        fs::read_to_string(workspace_manifest_path)?.parse()?;

    let known_path_deps = find_path_deps(&build_context.cargo_metadata)?;

    let mut writer = SDistWriter::new(&build_context.out, metadata21, excludes)?;
    let root_dir = PathBuf::from(format!(
        "{}-{}",
        &metadata21.get_distribution_escaped(),
        &metadata21.get_version_escaped()
    ));

    // Add local path dependencies
    let mut path_dep_workspace_manifests = HashMap::new();
    for (name, path_dep) in known_path_deps.iter() {
        // Path dependencies may not be in the same workspace as the root crate,
        // thus we need to find out its workspace root from `cargo metadata`
        let path_dep_metadata = MetadataCommand::new()
            .manifest_path(path_dep)
            .verbose(true)
            // We don't need to resolve the dependency graph
            .no_deps()
            .exec()
            .with_context(|| {
                format!(
                    "Cargo metadata failed for {} at '{}'",
                    name,
                    path_dep.display()
                )
            })?;
        let path_dep_workspace_manifest =
            if path_dep_metadata.workspace_root == build_context.cargo_metadata.workspace_root {
                &workspace_manifest
            } else {
                if !path_dep_workspace_manifests.contains_key(&path_dep_metadata.workspace_root) {
                    let manifest: toml_edit::Document =
                        fs::read_to_string(path_dep_metadata.workspace_root.join("Cargo.toml"))?
                            .parse()?;
                    path_dep_workspace_manifests
                        .insert(path_dep_metadata.workspace_root.clone(), manifest);
                }
                &path_dep_workspace_manifests[&path_dep_metadata.workspace_root]
            };
        add_crate_to_source_distribution(
            &mut writer,
            &pyproject_toml_path,
            path_dep,
            path_dep_workspace_manifest,
            &root_dir.join(LOCAL_DEPENDENCIES_FOLDER).join(name),
            &known_path_deps,
            false,
        )
        .context(format!(
            "Failed to add local dependency {} at {} to the source distribution",
            name,
            path_dep.display()
        ))?;
    }

    // Add the main crate
    add_crate_to_source_distribution(
        &mut writer,
        &pyproject_toml_path,
        manifest_path,
        &workspace_manifest,
        &root_dir,
        &known_path_deps,
        true,
    )?;

    let abs_manifest_path = manifest_path
        .normalize()
        .with_context(|| format!("failed to normalize path `{}`", manifest_path.display()))?
        .into_path_buf();
    let abs_manifest_dir = abs_manifest_path.parent().unwrap();
    let cargo_lock_path = abs_manifest_dir.join("Cargo.lock");
    let cargo_lock_exists = cargo_lock_path.exists();
    let workspace_cargo_lock = build_context
        .cargo_metadata
        .workspace_root
        .join("Cargo.lock");
    let workspace_cargo_lock_exists = workspace_cargo_lock.exists();
    let cargo_lock_required =
        build_context.cargo_options.locked || build_context.cargo_options.frozen;
    if cargo_lock_required || cargo_lock_exists || workspace_cargo_lock_exists {
        let project_root = pyproject_toml_path.parent().unwrap();
        let relative_cargo_lock = if cargo_lock_path.starts_with(project_root) {
            cargo_lock_path.strip_prefix(project_root).unwrap()
        } else {
            cargo_lock_path.strip_prefix(abs_manifest_dir).unwrap()
        };
        if cargo_lock_exists {
            writer.add_file(root_dir.join(relative_cargo_lock), &cargo_lock_path)?;
        } else {
            // Fallback to workspace Cargo lock file
            writer.add_file(root_dir.join(relative_cargo_lock), workspace_cargo_lock)?;
        }
    } else {
        eprintln!(
            "⚠️  Warning: Cargo.lock is not found, it is recommended \
            to include it in the source distribution"
        );
    }

    let pyproject_dir = pyproject_toml_path.parent().unwrap();
    // Add python source files
    let mut python_packages = Vec::new();
    if let Some(python_module) = build_context.project_layout.python_module.as_ref() {
        python_packages.push(python_module.to_path_buf());
    }
    for package in &build_context.project_layout.python_packages {
        let package_path = build_context.project_layout.python_dir.join(package);
        if python_packages.iter().any(|p| *p == package_path) {
            continue;
        }
        python_packages.push(package_path);
    }

    for package in python_packages {
        for entry in ignore::Walk::new(package) {
            let source = entry?.into_path();
            // Technically, `ignore` crate should handle this,
            // but somehow it doesn't on Alpine Linux running in GitHub Actions,
            // so we do it manually here.
            // See https://github.com/PyO3/maturin/pull/1187#issuecomment-1273987013
            if source
                .extension()
                .map(|ext| ext == "pyc" || ext == "pyd" || ext == "so")
                .unwrap_or_default()
            {
                debug!("Ignoring {}", source.display());
                continue;
            }
            let target = root_dir.join(source.strip_prefix(pyproject_dir).unwrap());
            if source.is_dir() {
                writer.add_directory(target)?;
            } else {
                writer.add_file(target, &source)?;
            }
        }
    }

    // Add readme, license
    if let Some(project) = pyproject.project.as_ref() {
        if let Some(pyproject_toml::ReadMe::RelativePath(readme)) = project.readme.as_ref() {
            writer.add_file(root_dir.join(readme), pyproject_dir.join(readme))?;
        }
        if let Some(pyproject_toml::License {
            file: Some(license),
            text: None,
        }) = project.license.as_ref()
        {
            writer.add_file(root_dir.join(license), pyproject_dir.join(license))?;
        }
    }

    let mut include = |pattern| -> Result<()> {
        println!("📦 Including files matching \"{pattern}\"");
        for source in glob::glob(&pyproject_dir.join(pattern).to_string_lossy())
            .expect("No files found for pattern")
            .filter_map(Result::ok)
        {
            let target = root_dir.join(source.strip_prefix(pyproject_dir).unwrap());
            if source.is_dir() {
                writer.add_directory(target)?;
            } else {
                writer.add_file(target, source)?;
            }
        }
        Ok(())
    };

    if let Some(glob_patterns) = pyproject.include() {
        for pattern in glob_patterns
            .iter()
            .filter_map(|glob_pattern| glob_pattern.targets(Format::Sdist))
        {
            include(pattern)?;
        }
    }

    writer.add_bytes(
        root_dir.join("PKG-INFO"),
        metadata21.to_file_contents()?.as_bytes(),
    )?;

    add_data(&mut writer, build_context.project_layout.data.as_deref())?;
    let source_distribution_path = writer.finish()?;

    println!(
        "📦 Built source distribution to {}",
        source_distribution_path.display()
    );

    Ok(source_distribution_path)
}
