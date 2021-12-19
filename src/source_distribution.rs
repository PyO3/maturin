use crate::module_writer::ModuleWriter;
use crate::{Metadata21, SDistWriter};
use anyhow::{bail, Context, Result};
use cargo_metadata::{Metadata, PackageId};
use fs_err as fs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str;

const LOCAL_DEPENDENCIES_FOLDER: &str = "local_dependencies";

#[derive(Debug, Clone)]
struct PathDependency {
    id: PackageId,
    path: PathBuf,
}

/// We need cargo to load the local dependencies from the location where we put them in the source
/// distribution. Since there is no cargo-backed way to replace dependencies
/// (see https://github.com/rust-lang/cargo/issues/9170), we do a simple
/// Cargo.toml rewrite ourselves.
/// A big chunk of that comes from cargo edit, and esp.
/// https://github.com/killercup/cargo-edit/blob/2a08f0311bcb61690d71d39cb9e55e69b256c8e1/src/manifest.rs
/// This method is rather frail, but unfortunately I don't know a better solution.
fn rewrite_cargo_toml(
    manifest_path: impl AsRef<Path>,
    known_path_deps: &HashMap<String, PathDependency>,
    root_crate: bool,
) -> Result<String> {
    let text = fs::read_to_string(&manifest_path).context(format!(
        "Can't read Cargo.toml at {}",
        manifest_path.as_ref().display(),
    ))?;
    let mut data = toml::from_str::<toml::value::Table>(&text).context(format!(
        "Failed to parse Cargo.toml at {}",
        manifest_path.as_ref().display()
    ))?;
    let mut rewritten = false;
    //  ˇˇˇˇˇˇˇˇˇˇˇˇ dep_category
    // [dependencies]
    // some_path_dep = { path = "../some_path_dep" }
    //                          ^^^^^^^^^^^^^^^^^^ table[&dep_name]["path"]
    // ^^^^^^^^^^^^^ dep_name
    for dep_category in &["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = data.get_mut(*dep_category).and_then(|x| x.as_table_mut()) {
            let dep_names: Vec<_> = table.iter().map(|(key, _)| key.to_string()).collect();
            for dep_name in dep_names {
                // There should either be no value for path, or it should be a string
                if table.get(&dep_name).and_then(|x| x.get("path")).is_none() {
                    continue;
                }
                if !table[&dep_name]["path"].is_str() {
                    bail!(
                        "In {}, {} {} has a path value that is not a string",
                        manifest_path.as_ref().display(),
                        dep_category,
                        dep_name
                    )
                }
                // This is the location of the targeted crate in the source distribution
                table[&dep_name]["path"] = if root_crate {
                    format!("{}/{}", LOCAL_DEPENDENCIES_FOLDER, dep_name).into()
                } else {
                    // Cargo.toml contains relative paths, and we're already in LOCAL_DEPENDENCIES_FOLDER
                    format!("../{}", dep_name).into()
                };
                rewritten = true;
                if !known_path_deps.contains_key(&dep_name) {
                    bail!(
                        "cargo metadata does not know about the path for {}.{} present in {}, \
                        which should never happen ಠ_ಠ",
                        dep_category,
                        dep_name,
                        manifest_path.as_ref().display()
                    );
                }
            }
        }
    }
    if rewritten {
        Ok(toml::to_string(&data)?)
    } else {
        Ok(text)
    }
}

/// Copies the files of a crate to a source distribution, recursively adding path dependencies
/// and rewriting path entries in Cargo.toml
///
/// Runs `cargo package --list --allow-dirty` to obtain a list of files to package.
fn add_crate_to_source_distribution(
    writer: &mut SDistWriter,
    manifest_path: impl AsRef<Path>,
    prefix: impl AsRef<Path>,
    known_path_deps: &HashMap<String, PathDependency>,
    root_crate: bool,
) -> Result<()> {
    let output = Command::new("cargo")
        .args(&["package", "--list", "--allow-dirty", "--manifest-path"])
        .arg(manifest_path.as_ref())
        .output()
        .context("Failed to run cargo")?;
    if !output.status.success() {
        bail!(
            "Failed to query file list from cargo: {}\n--- Stdout:\n{}\n--- Stderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let file_list: Vec<&Path> = str::from_utf8(&output.stdout)
        .context("Cargo printed invalid utf-8 ಠ_ಠ")?
        .lines()
        .map(Path::new)
        .collect();

    let manifest_dir = manifest_path.as_ref().parent().unwrap();

    let target_source: Vec<(PathBuf, PathBuf)> = file_list
        .iter()
        .map(|relative_to_manifests| {
            let relative_to_cwd = manifest_dir.join(relative_to_manifests);
            (relative_to_manifests.to_path_buf(), relative_to_cwd)
        })
        // We rewrite Cargo.toml and add it separately
        .filter(|(target, source)| {
            // Skip generated files. See https://github.com/rust-lang/cargo/issues/7938#issuecomment-593280660
            // and https://github.com/PyO3/maturin/issues/449
            if target == Path::new("Cargo.toml.orig") || target == Path::new("Cargo.toml") {
                false
            } else {
                source.exists()
            }
        })
        .collect();

    if root_crate
        && !target_source
            .iter()
            .any(|(target, _)| target == Path::new("pyproject.toml"))
    {
        bail!(
            "pyproject.toml was not included by `cargo package`. \
                 Please make sure pyproject.toml is not excluded or build with `--no-sdist`"
        )
    }

    let rewritten_cargo_toml = rewrite_cargo_toml(&manifest_path, known_path_deps, root_crate)?;

    writer.add_directory(&prefix)?;
    writer.add_bytes(
        prefix
            .as_ref()
            .join(manifest_path.as_ref().file_name().unwrap()),
        rewritten_cargo_toml.as_bytes(),
    )?;
    for (target, source) in target_source {
        writer.add_file(prefix.as_ref().join(target), source)?;
    }

    Ok(())
}

/// Get path dependencies for a cargo package
fn get_path_deps(
    cargo_metadata: &Metadata,
    resolve: &cargo_metadata::Resolve,
    pkg_id: &cargo_metadata::PackageId,
    visited: &HashMap<String, PathDependency>,
) -> Result<HashMap<String, PathDependency>> {
    // Parse ids in the format:
    // on unix:    some_path_dep 0.1.0 (path+file:///home/konsti/maturin/test-crates/some_path_dep)
    // on windows: some_path_dep 0.1.0 (path+file:///C:/konsti/maturin/test-crates/some_path_dep)
    // This is not a good way to identify path dependencies, but I don't know a better one
    let node = resolve
        .nodes
        .iter()
        .find(|node| &node.id == pkg_id)
        .context("Expected to get a node of dependency graph from cargo")?;
    let path_deps = node
        .deps
        .iter()
        .filter(|node| node.pkg.repr.contains("path+file://"))
        .filter_map(|node| {
            cargo_metadata.packages.iter().find_map(|pkg| {
                if pkg.id.repr == node.pkg.repr && !visited.contains_key(&pkg.name) {
                    let path_dep = PathDependency {
                        id: pkg.id.clone(),
                        path: PathBuf::from(&pkg.manifest_path),
                    };
                    Some((pkg.name.clone(), path_dep))
                } else {
                    None
                }
            })
        })
        .collect();
    Ok(path_deps)
}

/// Finds all path dependencies of the crate
fn find_path_deps(cargo_metadata: &Metadata) -> Result<HashMap<String, PathDependency>> {
    let resolve = cargo_metadata
        .resolve
        .as_ref()
        .context("Expected to get a dependency graph from cargo")?;
    let root = resolve
        .root
        .clone()
        .context("Expected to get a root package id of dependency graph from cargo")?;
    let mut known_path_deps = HashMap::new();
    let mut stack = vec![root];
    while let Some(pkg_id) = stack.pop() {
        let path_deps = get_path_deps(cargo_metadata, resolve, &pkg_id, &known_path_deps)?;
        if path_deps.is_empty() {
            continue;
        }
        stack.extend(path_deps.values().map(|dep| dep.id.clone()));
        known_path_deps.extend(path_deps);
    }
    Ok(known_path_deps)
}

/// Creates a source distribution, packing the root crate and all local dependencies
///
/// The source distribution format is specified in
/// [PEP 517 under "build_sdist"](https://www.python.org/dev/peps/pep-0517/#build-sdist)
/// and in
/// https://packaging.python.org/specifications/source-distribution-format/#source-distribution-file-format
pub fn source_distribution(
    wheel_dir: impl AsRef<Path>,
    metadata21: &Metadata21,
    manifest_path: impl AsRef<Path>,
    cargo_metadata: &Metadata,
    sdist_include: Option<&Vec<String>>,
    include_cargo_lock: bool,
) -> Result<PathBuf> {
    let known_path_deps = find_path_deps(cargo_metadata)?;

    let mut writer = SDistWriter::new(wheel_dir, metadata21)?;
    let root_dir = PathBuf::from(format!(
        "{}-{}",
        &metadata21.get_distribution_escaped(),
        &metadata21.get_version_escaped()
    ));

    // Add local path dependencies
    for (name, path_dep) in known_path_deps.iter() {
        add_crate_to_source_distribution(
            &mut writer,
            &path_dep.path,
            &root_dir.join(LOCAL_DEPENDENCIES_FOLDER).join(name),
            &known_path_deps,
            false,
        )
        .context(format!(
            "Failed to add local dependency {} at {} to the source distribution",
            name,
            path_dep.path.display()
        ))?;
    }

    // Add the main crate
    add_crate_to_source_distribution(
        &mut writer,
        &manifest_path,
        &root_dir,
        &known_path_deps,
        true,
    )?;

    let manifest_dir = manifest_path.as_ref().parent().unwrap();
    if include_cargo_lock {
        let cargo_lock_path = manifest_dir.join("Cargo.lock");
        let target = root_dir.join("Cargo.lock");
        writer.add_file(&target, &cargo_lock_path)?;
    }

    if let Some(include_targets) = sdist_include {
        for pattern in include_targets {
            println!("📦 Including files matching \"{}\"", pattern);
            for source in glob::glob(&manifest_dir.join(pattern).to_string_lossy())
                .expect("No files found for pattern")
                .filter_map(Result::ok)
            {
                let target = root_dir.join(&source.strip_prefix(manifest_dir)?);
                writer.add_file(target, source)?;
            }
        }
    }

    writer.add_bytes(
        root_dir.join("PKG-INFO"),
        metadata21.to_file_contents().as_bytes(),
    )?;

    let source_distribution_path = writer.finish()?;

    println!(
        "📦 Built source distribution to {}",
        source_distribution_path.display()
    );

    Ok(source_distribution_path)
}
