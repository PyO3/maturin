use crate::pyproject_toml::{Format, SdistGenerator};
use crate::{BuildContext, ModuleWriter, PyProjectToml, SDistWriter, VirtualWriter};
use anyhow::{Context, Result, bail};
use cargo_metadata::camino::Utf8Path;
use cargo_metadata::{Metadata, MetadataCommand, PackageId};
use fs_err as fs;
use ignore::overrides::Override;
use normpath::PathExt as _;
use path_slash::PathExt as _;
use pyproject_toml::check_pep639_glob;
use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str;
use toml_edit::DocumentMut;
use tracing::{debug, trace, warn};

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
    manifest_path: PathBuf,
    /// workspace root of the path dependency
    workspace_root: PathBuf,
    /// readme path of the path dependency
    readme: Option<PathBuf>,
}

fn parse_toml_file(path: &Path, kind: &str) -> Result<toml_edit::DocumentMut> {
    let text = fs::read_to_string(path)?;
    let document = text.parse::<toml_edit::DocumentMut>().context(format!(
        "Failed to parse {} at {}",
        kind,
        path.display()
    ))?;
    Ok(document)
}

/// Rewrite Cargo.toml to only retain path dependencies that are actually used
///
/// We only want to add path dependencies that are actually used
/// to reduce the size of the source distribution.
fn rewrite_cargo_toml(
    document: &mut DocumentMut,
    manifest_path: &Path,
    known_path_deps: &HashMap<String, PathDependency>,
) -> Result<()> {
    debug!(
        "Rewriting Cargo.toml `workspace.members` at {}",
        manifest_path.display()
    );
    // Update workspace members
    if let Some(workspace) = document.get_mut("workspace").and_then(|x| x.as_table_mut()) {
        if let Some(members) = workspace.get_mut("members").and_then(|x| x.as_array()) {
            if known_path_deps.is_empty() {
                // Remove workspace members when there isn't any path dep
                workspace.remove("members");
                if workspace.is_empty() {
                    // Remove workspace all together if it's empty
                    document.remove("workspace");
                }
            } else {
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
                            if known_path_deps.values().any(|path_dep| {
                                let relative_path = path_dep
                                    .manifest_path
                                    .strip_prefix(&path_dep.workspace_root)
                                    .unwrap();
                                let relative_path_str = relative_path.to_str().unwrap();
                                pattern.matches(relative_path_str)
                            }) {
                                new_members.push(member_path);
                            }
                        } else if known_path_deps.contains_key(member_path) {
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
    }
    Ok(())
}

/// Rewrite `Cargo.toml` to find the readme in the same directory.
///
/// `package.readme` may point to any point above the package, so when we move the directory, but
/// keep the readme position, we could get different readme files at the same archive location.
/// Putting the readme in the same directory as the `Cargo.toml` prevents this.
fn rewrite_cargo_toml_readme(
    document: &mut DocumentMut,
    manifest_path: &Path,
    readme_name: Option<&str>,
) -> Result<()> {
    debug!(
        "Rewriting Cargo.toml `package.readme` at {}",
        manifest_path.display()
    );

    if let Some(readme_name) = readme_name {
        let project = document.get_mut("package").with_context(|| {
            format!(
                "Missing `[package]` table in Cargo.toml with readme at {}",
                manifest_path.display()
            )
        })?;
        project["readme"] = toml_edit::value(readme_name);
    }
    Ok(())
}

/// When `pyproject.toml` is inside the Cargo workspace root,
/// we need to update `tool.maturin.manifest-path` in `pyproject.toml`.
fn rewrite_pyproject_toml(
    pyproject_toml_path: &Path,
    relative_manifest_path: &Path,
) -> Result<String> {
    let mut data = parse_toml_file(pyproject_toml_path, "pyproject.toml")?;
    let tool = data
        .entry("tool")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_like_mut()
        .with_context(|| {
            format!(
                "`[tool]` must be a table in {}",
                pyproject_toml_path.display()
            )
        })?;
    let maturin = tool
        .entry("maturin")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_like_mut()
        .with_context(|| {
            format!(
                "`[tool.maturin]` must be a table in {}",
                pyproject_toml_path.display()
            )
        })?;

    maturin.remove("manifest-path");
    maturin.insert(
        "manifest-path",
        toml_edit::value(relative_manifest_path.to_str().unwrap()),
    );

    Ok(data.to_string())
}

/// Copies the files of a crate to a source distribution, recursively adding path dependencies
/// and rewriting path entries in Cargo.toml
///
/// Runs `cargo package --list --allow-dirty` to obtain a list of files to package.
fn add_crate_to_source_distribution(
    writer: &mut VirtualWriter<SDistWriter>,
    manifest_path: impl AsRef<Path>,
    prefix: impl AsRef<Path>,
    readme: Option<&Path>,
    known_path_deps: &HashMap<String, PathDependency>,
    root_crate: bool,
    skip_cargo_toml: bool,
) -> Result<()> {
    debug!(
        "Getting cargo package file list for {}",
        manifest_path.as_ref().display()
    );
    let prefix = prefix.as_ref();
    let manifest_path = manifest_path.as_ref();
    let args = ["package", "--list", "--allow-dirty", "--manifest-path"];
    let output = Command::new("cargo")
        .args(args)
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
    if !output.stderr.is_empty() {
        eprintln!(
            "From `cargo {} {}`:",
            args.join(" "),
            manifest_path.display()
        );
        std::io::stderr().write_all(&output.stderr)?;
    }

    let file_list: Vec<&str> = str::from_utf8(&output.stdout)
        .context("Cargo printed invalid utf-8 ಠ_ಠ")?
        .lines()
        .collect();

    trace!("File list: {}", file_list.join(", "));

    // manifest_dir should be a relative path
    let manifest_dir = manifest_path.parent().unwrap();
    let target_source: Vec<_> = file_list
        .into_iter()
        .map(|relative_to_manifests| {
            let relative_to_cwd = manifest_dir.join(relative_to_manifests);
            (relative_to_manifests, relative_to_cwd)
        })
        .filter(|(target, source)| {
            #[allow(clippy::if_same_then_else)]
            if *target == "Cargo.toml.orig" {
                // Skip generated files. See https://github.com/rust-lang/cargo/issues/7938#issuecomment-593280660
                // and https://github.com/PyO3/maturin/issues/449
                false
            } else if *target == "Cargo.toml" {
                // We rewrite Cargo.toml and add it separately
                false
            } else if root_crate && *target == "pyproject.toml" {
                // pyproject.toml is handled separately because it has to be put in the root dir
                // of source distribution
                false
            } else if prefix.components().count() == 1 && *target == "pyproject.toml" {
                // Skip pyproject.toml for cases when the root is in a workspace member and both the
                // member and the root have a pyproject.toml.
                debug!("Skipping potentially non-main {}", prefix.join(target).display());
                false
            } else if matches!(Path::new(target).extension(), Some(ext) if ext == "pyc" || ext == "pyd" || ext == "so") {
                // Technically, `cargo package --list` should handle this,
                // but somehow it doesn't on Alpine Linux running in GitHub Actions,
                // so we do it manually here.
                // See https://github.com/PyO3/maturin/pull/1255#issuecomment-1308838786
                debug!("Ignoring {}", target);
                false
            } else {
                source.exists()
            }
        })
        .collect();

    let cargo_toml_path = prefix.join(manifest_path.file_name().unwrap());

    let readme_name = readme
        .as_ref()
        .map(|readme| {
            readme
                .file_name()
                .and_then(OsStr::to_str)
                .with_context(|| format!("Missing readme filename for {}", manifest_path.display()))
        })
        .transpose()?;

    if root_crate {
        let mut document = parse_toml_file(manifest_path, "Cargo.toml")?;
        rewrite_cargo_toml_readme(&mut document, manifest_path, readme_name)?;
        rewrite_cargo_toml(&mut document, manifest_path, known_path_deps)?;
        writer.add_bytes(
            cargo_toml_path,
            Some(manifest_path),
            document.to_string().as_bytes(),
            false,
        )?;
    } else if !skip_cargo_toml {
        let mut document = parse_toml_file(manifest_path, "Cargo.toml")?;
        rewrite_cargo_toml_readme(&mut document, manifest_path, readme_name)?;
        writer.add_bytes(
            cargo_toml_path,
            Some(manifest_path),
            document.to_string().as_bytes(),
            false,
        )?;
    }

    for (target, source) in target_source {
        writer.add_file(prefix.join(target), source, false)?;
    }

    Ok(())
}

/// Finds all path dependencies of the crate
pub fn find_path_deps(cargo_metadata: &Metadata) -> Result<HashMap<String, PathDependency>> {
    let root = cargo_metadata
        .root_package()
        .context("Expected the dependency graph to have a root package")?;
    let pkg_readmes = cargo_metadata
        .packages
        .iter()
        .filter_map(|package| {
            package
                .readme
                .as_ref()
                .map(|readme| (package.id.clone(), readme.clone().into_std_path_buf()))
        })
        .collect::<HashMap<PackageId, PathBuf>>();
    // scan the dependency graph for path dependencies
    let mut path_deps = HashMap::new();
    let mut stack: Vec<&cargo_metadata::Package> = vec![root];
    while let Some(top) = stack.pop() {
        // There seems to be no API to get the package id of a dependency, so collect the package ids from resolve
        // and match them up with the `Dependency`s from `Package.dependencies`.
        let dep_ids = &cargo_metadata
            .resolve
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|node| node.id == top.id)
            .unwrap()
            .dependencies;
        for dep_id in dep_ids {
            // Assumption: Each package name can only occur once in a `[dependencies]` table.
            let dependency = top
                .dependencies
                .iter()
                .find(|&package| {
                    // Package ids are opaque and there seems to be no way to query their name.
                    let dep_name = &cargo_metadata
                        .packages
                        .iter()
                        .find(|package| &package.id == dep_id)
                        .unwrap()
                        .name;
                    package.name == dep_name.as_ref()
                })
                .unwrap();
            if let Some(path) = &dependency.path {
                let dep_name = dependency.rename.as_ref().unwrap_or(&dependency.name);
                if path_deps.contains_key(dep_name) {
                    continue;
                }
                // we search for the respective package by `manifest_path`, there seems
                // to be no way to query the dependency graph given `dependency`
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
                            "Failed to resolve workspace root for {dep_id} at '{dep_manifest_path}'"
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
                        readme: pkg_readmes.get(dep_id).cloned(),
                    },
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

/// Copies the files of git to a source distribution
///
/// Runs `git ls-files -z` to obtain a list of files to package.
fn add_git_tracked_files_to_sdist(
    pyproject_toml_path: &Path,
    writer: &mut VirtualWriter<SDistWriter>,
    prefix: impl AsRef<Path>,
) -> Result<()> {
    let pyproject_dir = pyproject_toml_path.parent().unwrap();
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(pyproject_dir)
        .output()
        .context("Failed to run `git ls-files -z`")?;
    if !output.status.success() {
        bail!(
            "Failed to query file list from git: {}\n--- Project Path: {}\n--- Stdout:\n{}\n--- Stderr:\n{}",
            output.status,
            pyproject_dir.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let prefix = prefix.as_ref();
    let file_paths = str::from_utf8(&output.stdout)
        .context("git printed invalid utf-8 ಠ_ಠ")?
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(Path::new);
    for source in file_paths {
        writer.add_file(prefix.join(source), pyproject_dir.join(source), false)?;
    }
    Ok(())
}

/// Copies the files of a crate to a source distribution, recursively adding path dependencies
/// and rewriting path entries in Cargo.toml
fn add_cargo_package_files_to_sdist(
    build_context: &BuildContext,
    pyproject_toml_path: &Path,
    writer: &mut VirtualWriter<SDistWriter>,
    root_dir: &Path,
) -> Result<()> {
    let manifest_path = &build_context.manifest_path;
    let workspace_root = &build_context.cargo_metadata.workspace_root;
    let workspace_manifest_path = workspace_root.join("Cargo.toml");

    let known_path_deps = find_path_deps(&build_context.cargo_metadata)?;
    debug!(
        "Found path dependencies: {:?}",
        known_path_deps.keys().collect::<Vec<_>>()
    );
    let mut sdist_root =
        common_path_prefix(workspace_root.as_std_path(), pyproject_toml_path).unwrap();
    for path_dep in known_path_deps.values() {
        if let Some(prefix) =
            common_path_prefix(&sdist_root, path_dep.manifest_path.parent().unwrap())
        {
            sdist_root = prefix;
        } else {
            bail!("Failed to determine common path prefix of path dependencies");
        }
    }

    debug!("Found sdist root: {}", sdist_root.display());

    // Add local path dependencies
    for (name, path_dep) in known_path_deps.iter() {
        add_path_dep(
            writer,
            root_dir,
            workspace_root,
            &workspace_manifest_path,
            &known_path_deps,
            &sdist_root,
            name,
            path_dep,
        )
        .with_context(|| format!("Failed to add path dependency {name}"))?;
    }

    debug!("Adding the main crate {}", manifest_path.display());
    // Add the main crate
    let abs_manifest_path = manifest_path
        .normalize()
        .with_context(|| {
            format!(
                "manifest path `{}` does not exist or is invalid",
                manifest_path.display()
            )
        })?
        .into_path_buf();
    let abs_manifest_dir = abs_manifest_path.parent().unwrap();
    let main_crate = build_context.cargo_metadata.root_package().unwrap();
    let relative_main_crate_manifest_dir = manifest_path
        .parent()
        .unwrap()
        .strip_prefix(&sdist_root)
        .unwrap();
    // Handle possible relative readme field in Cargo.toml
    let readme_path = if let Some(readme) = main_crate.readme.as_ref() {
        let readme = abs_manifest_dir.join(readme);
        let abs_readme = readme
            .normalize()
            .with_context(|| {
                format!(
                    "readme path `{}` does not exist or is invalid",
                    readme.display()
                )
            })?
            .into_path_buf();
        // Add readme next to Cargo.toml so we don't get collisions between crates using readmes
        // higher up the file tree.
        writer.add_file(
            root_dir
                .join(relative_main_crate_manifest_dir)
                .join(readme.file_name().unwrap()),
            &abs_readme,
            false,
        )?;
        Some(abs_readme)
    } else {
        None
    };
    add_crate_to_source_distribution(
        writer,
        manifest_path,
        root_dir.join(relative_main_crate_manifest_dir),
        readme_path.as_deref(),
        &known_path_deps,
        true,
        false,
    )?;

    // Add Cargo.lock file and workspace Cargo.toml
    let manifest_cargo_lock_path = abs_manifest_dir.join("Cargo.lock");
    let workspace_cargo_lock = workspace_root.join("Cargo.lock").into_std_path_buf();
    let (cargo_lock_path, use_workspace_cargo_lock) = if manifest_cargo_lock_path.exists() {
        (Some(manifest_cargo_lock_path.clone()), false)
    } else if workspace_cargo_lock.exists() {
        (Some(workspace_cargo_lock), true)
    } else {
        (None, false)
    };
    let cargo_lock_required =
        build_context.cargo_options.locked || build_context.cargo_options.frozen;
    if let Some(cargo_lock_path) = cargo_lock_path {
        let pyproject_root = pyproject_toml_path.parent().unwrap();
        let project_root =
            if pyproject_root == sdist_root || pyproject_root.starts_with(&sdist_root) {
                &sdist_root
            } else {
                assert!(sdist_root.starts_with(pyproject_root));
                pyproject_root
            };
        let relative_cargo_lock = cargo_lock_path.strip_prefix(project_root).unwrap();
        writer.add_file(root_dir.join(relative_cargo_lock), &cargo_lock_path, false)?;
        if use_workspace_cargo_lock {
            let relative_workspace_cargo_toml = relative_cargo_lock.with_file_name("Cargo.toml");
            let mut deps_to_keep = known_path_deps.clone();
            // Also need to the main Python binding crate
            let main_member_name = abs_manifest_dir
                .strip_prefix(workspace_root)
                .unwrap()
                .to_slash()
                .unwrap()
                .to_string();
            deps_to_keep.insert(
                main_member_name,
                PathDependency {
                    manifest_path: manifest_path.clone(),
                    workspace_root: workspace_root.clone().into_std_path_buf(),
                    readme: None,
                },
            );
            let mut document =
                parse_toml_file(workspace_manifest_path.as_std_path(), "Cargo.toml")?;
            rewrite_cargo_toml(
                &mut document,
                workspace_manifest_path.as_std_path(),
                &deps_to_keep,
            )?;
            writer.add_bytes(
                root_dir.join(relative_workspace_cargo_toml),
                Some(workspace_manifest_path.as_std_path()),
                document.to_string().as_bytes(),
                false,
            )?;
        }
    } else if cargo_lock_required {
        bail!("Cargo.lock is required by `--locked`/`--frozen` but it's not found.");
    } else {
        eprintln!(
            "⚠️  Warning: Cargo.lock is not found, it is recommended \
            to include it in the source distribution"
        );
    }

    // Add pyproject.toml
    let pyproject_dir = pyproject_toml_path.parent().unwrap();
    if pyproject_dir != sdist_root {
        // rewrite `tool.maturin.manifest-path` in pyproject.toml
        let rewritten_pyproject_toml = rewrite_pyproject_toml(
            pyproject_toml_path,
            &relative_main_crate_manifest_dir.join("Cargo.toml"),
        )?;
        writer.add_bytes(
            root_dir.join("pyproject.toml"),
            Some(pyproject_toml_path),
            rewritten_pyproject_toml.as_bytes(),
            false,
        )?;
    } else {
        writer.add_file(root_dir.join("pyproject.toml"), pyproject_toml_path, false)?;
    }

    // Add python source files
    let mut python_packages = Vec::new();
    if let Some(python_module) = build_context.project_layout.python_module.as_ref() {
        trace!("Resolved python module: {}", python_module.display());
        python_packages.push(python_module.to_path_buf());
    }
    for package in &build_context.project_layout.python_packages {
        let package_path = build_context.project_layout.python_dir.join(package);
        if python_packages.contains(&package_path) {
            continue;
        }
        trace!("Resolved python package: {}", package_path.display());
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
            if !source.is_dir() {
                writer.add_file(target, &source, false)?;
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)] // TODO(konsti)
fn add_path_dep(
    writer: &mut VirtualWriter<SDistWriter>,
    root_dir: &Path,
    workspace_root: &Utf8Path,
    workspace_manifest_path: &Utf8Path,
    known_path_deps: &HashMap<String, PathDependency>,
    sdist_root: &Path,
    name: &str,
    path_dep: &PathDependency,
) -> Result<()> {
    debug!(
        "Adding path dependency: {} at {}",
        name,
        path_dep.manifest_path.display()
    );
    let path_dep_manifest_dir = path_dep.manifest_path.parent().unwrap();
    let relative_path_dep_manifest_dir = path_dep_manifest_dir.strip_prefix(sdist_root).unwrap();
    // we may need to rewrite workspace Cargo.toml later so don't add it to sdist yet
    let skip_cargo_toml = workspace_manifest_path == path_dep.manifest_path;
    add_crate_to_source_distribution(
        writer,
        &path_dep.manifest_path,
        root_dir.join(relative_path_dep_manifest_dir),
        path_dep.readme.as_deref(),
        known_path_deps,
        false,
        skip_cargo_toml,
    )
    .with_context(|| {
        format!(
            "Failed to add local dependency {} at {} to the source distribution",
            name,
            path_dep.manifest_path.display()
        )
    })?;
    // Include readme
    if let Some(readme) = path_dep.readme.as_ref() {
        let readme = path_dep_manifest_dir.join(readme);
        let abs_readme = readme
            .normalize()
            .with_context(|| {
                format!(
                    "readme path `{}` does not exist or is invalid",
                    readme.display()
                )
            })?
            .into_path_buf();
        // Add readme next to Cargo.toml so we don't get collisions between crates using readmes
        // higher up the file tree. See also [`rewrite_cargo_toml_readme`].
        writer.add_file(
            root_dir
                .join(relative_path_dep_manifest_dir)
                .join(readme.file_name().unwrap()),
            &abs_readme,
            false,
        )?;
    }
    // Handle different workspace manifest
    if path_dep.workspace_root != workspace_root {
        let path_dep_workspace_manifest = path_dep.workspace_root.join("Cargo.toml");
        let relative_path_dep_workspace_manifest = path_dep_workspace_manifest
            .strip_prefix(sdist_root)
            .unwrap();
        writer.add_file(
            root_dir.join(relative_path_dep_workspace_manifest),
            &path_dep_workspace_manifest,
            false,
        )?;
    }
    Ok(())
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
    excludes: Override,
) -> Result<PathBuf> {
    let pyproject_toml_path = build_context
        .pyproject_toml_path
        .normalize()
        .with_context(|| {
            format!(
                "pyproject.toml path `{}` does not exist or is invalid",
                build_context.pyproject_toml_path.display()
            )
        })?
        .into_path_buf();

    let source_date_epoch: Option<u64> =
        env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|var| match var.parse() {
                Err(_) => {
                    warn!("SOURCE_DATE_EPOCH is malformed, ignoring");
                    None
                }
                Ok(val) => Some(val),
            });

    let metadata24 = &build_context.metadata24;
    let writer = SDistWriter::new(&build_context.out, metadata24, source_date_epoch)?;
    let mut writer = VirtualWriter::new(writer, excludes);
    let root_dir = PathBuf::from(format!(
        "{}-{}",
        &metadata24.get_distribution_escaped(),
        &metadata24.get_version_escaped()
    ));

    match pyproject.sdist_generator() {
        SdistGenerator::Cargo => add_cargo_package_files_to_sdist(
            build_context,
            &pyproject_toml_path,
            &mut writer,
            &root_dir,
        )?,
        SdistGenerator::Git => {
            add_git_tracked_files_to_sdist(&pyproject_toml_path, &mut writer, &root_dir)?
        }
    }

    let pyproject_toml_path = build_context
        .pyproject_toml_path
        .normalize()
        .with_context(|| {
            format!(
                "pyproject.toml path `{}` does not exist or is invalid",
                build_context.pyproject_toml_path.display()
            )
        })?
        .into_path_buf();
    let pyproject_dir = pyproject_toml_path.parent().unwrap();
    // Add readme, license
    if let Some(project) = pyproject.project.as_ref() {
        if let Some(pyproject_toml::ReadMe::RelativePath(readme)) = project.readme.as_ref() {
            writer.add_file(root_dir.join(readme), pyproject_dir.join(readme), false)?;
        }
        if let Some(pyproject_toml::License::File { file }) = project.license.as_ref() {
            writer.add_file(root_dir.join(file), pyproject_dir.join(file), false)?;
        }
        if let Some(license_files) = &project.license_files {
            // Safe on Windows and Unix as neither forward nor backwards slashes are escaped.
            let escaped_pyproject_dir =
                PathBuf::from(glob::Pattern::escape(pyproject_dir.to_str().unwrap()));
            let mut seen = HashSet::new();
            for license_glob in license_files {
                check_pep639_glob(license_glob)?;
                for license_path in
                    glob::glob(&escaped_pyproject_dir.join(license_glob).to_string_lossy())?
                {
                    let license_path = license_path?;
                    if !license_path.is_file() {
                        continue;
                    }
                    let license_path = license_path
                        .strip_prefix(pyproject_dir)
                        .expect("matched path starts with glob root")
                        .to_path_buf();
                    if seen.insert(license_path.clone()) {
                        debug!("Including license file `{}`", license_path.display());
                        writer.add_file(
                            root_dir.join(&license_path),
                            pyproject_dir.join(&license_path),
                            false,
                        )?;
                    }
                }
            }
        }
    }

    let mut include = |pattern| -> Result<()> {
        eprintln!("📦 Including files matching \"{pattern}\"");
        for source in glob::glob(&pyproject_dir.join(pattern).to_string_lossy())
            .with_context(|| format!("Invalid glob pattern: {pattern}"))?
            .filter_map(Result::ok)
        {
            let target = root_dir.join(source.strip_prefix(pyproject_dir).unwrap());
            if !source.is_dir() {
                writer.add_file(target, source, false)?;
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

    let pkg_info = root_dir.join("PKG-INFO");
    writer.add_bytes(
        &pkg_info,
        None,
        metadata24.to_file_contents()?.as_bytes(),
        false,
    )?;

    let source_distribution_path = writer.finish(&pkg_info)?;

    eprintln!(
        "📦 Built source distribution to {}",
        source_distribution_path.display()
    );

    Ok(source_distribution_path)
}

/// Find the common prefix, if any, between two paths
///
/// Taken from https://docs.rs/common-path/1.0.0/src/common_path/lib.rs.html#84-109
/// License: MIT/Apache 2.0
fn common_path_prefix<P, Q>(one: P, two: Q) -> Option<PathBuf>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    let one = one.as_ref();
    let two = two.as_ref();
    let one = one.components();
    let two = two.components();
    let mut final_path = PathBuf::new();
    let mut found = false;
    let paths = one.zip(two);
    for (l, r) in paths {
        if l == r {
            final_path.push(l.as_os_str());
            found = true;
        } else {
            break;
        }
    }
    if found { Some(final_path) } else { None }
}
