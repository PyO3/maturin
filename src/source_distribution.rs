use crate::module_writer::ModuleWriter;
use crate::{Metadata21, SDistWriter};
use anyhow::{bail, format_err, Context, Result};
use cargo_metadata::Metadata;
use fs_err as fs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str;

const LOCAL_DEPENDENCIES_FOLDER: &str = "local_dependencies";

/// We need cargo to load the local dependencies from the location where we put them in the source
/// distribution. Since there is no cargo-backed way to replace dependencies
/// (see https://github.com/rust-lang/cargo/issues/9170), we do a simple
/// Cargo.toml rewrite ourselves.
/// A big chunk of that (including toml_edit) comes from cargo edit, and esp.
/// https://github.com/killercup/cargo-edit/blob/2a08f0311bcb61690d71d39cb9e55e69b256c8e1/src/manifest.rs
/// This method is rather frail, but unfortunately I don't know a better solution.
fn rewrite_cargo_toml(
    manifest_path: impl AsRef<Path>,
    known_path_deps: &HashMap<&String, &PathBuf>,
    is_path_dep: bool,
) -> Result<String> {
    let text = fs::read_to_string(&manifest_path).context(format!(
        "Can't read Cargo.toml at {}",
        manifest_path.as_ref().display(),
    ))?;
    let mut data = text.parse::<toml_edit::Document>().context(format!(
        "Failed to parse Cargo.toml at {}",
        manifest_path.as_ref().display()
    ))?;
    //  ˇˇˇˇˇˇˇˇˇˇˇˇ dep_category
    // [dependencies]
    // some_path_dep = { path = "../some_path_dep" }
    //                          ^^^^^^^^^^^^^^^^^^ table[&dep_name]["path"]
    // ^^^^^^^^^^^^^ dep_name
    for dep_category in &["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = data[&dep_category].as_table_mut() {
            let dep_names: Vec<_> = table.iter().map(|(key, _)| key.to_string()).collect();
            for dep_name in dep_names {
                // There should either be no value for path, or it should be a string
                if table[&dep_name]["path"].is_none() {
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
                table[&dep_name]["path"] = toml_edit::value(if is_path_dep {
                    format!("../{}", dep_name)
                } else {
                    format!("{}/{}", LOCAL_DEPENDENCIES_FOLDER, dep_name)
                });
                if !known_path_deps.contains_key(&dep_name) {
                    bail!(
                        "cargo metadata does not know about the path for {} {} present in {}, which should never happen ಠ_ಠ",
                        dep_category, dep_name, manifest_path.as_ref().display()
                    );
                }
            }
        }
    }
    Ok(data.to_string_in_original_order())
}

/// Copies the files of a crate to a source distribution, recursively adding path dependencies
/// and rewriting path entries in Cargo.toml
///
/// Runs `cargo package --list --allow-dirty` to obtain a list of files to package.
fn add_crate_to_source_distribution(
    writer: &mut SDistWriter,
    manifest_path: impl AsRef<Path>,
    prefix: impl AsRef<Path>,
    known_path_deps: &HashMap<&String, &PathBuf>,
    root_crate: bool,
    is_path_dep: bool,
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
        .filter(|(target, _)| {
            target != Path::new("Cargo.toml.orig") && target != Path::new("Cargo.toml")
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

    let rewritten_cargo_toml = rewrite_cargo_toml(&manifest_path, known_path_deps, is_path_dep)?;

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

/// Finds all path dependencies of the crate.
fn find_path_deps(cargo_metadata: &Metadata) -> Result<HashMap<&String, &PathBuf>> {
    let root = cargo_metadata
        .root_package()
        .context("Expected the dependency graph to have a root package")?;
    // scan the dependency graph for path dependencies
    let mut path_deps = HashMap::new();
    let mut stack: Vec<&cargo_metadata::Package> = vec![root];
    while let Some(top) = stack.pop() {
        for dependency in &top.dependencies {
            if let Some(path) = &dependency.path {
                path_deps.insert(&dependency.name, path);
                // we search for the respective package by `manifest_path`, there seems
                // to be no way to query the dependency graph given `dependency`
                let dep_manifest_path = path.join("Cargo.toml");
                let dep_package = cargo_metadata
                    .packages
                    .iter()
                    .find(|package| package.manifest_path == dep_manifest_path)
                    .context(format!(
                        "Expected metadata to contain a package for path dependency {:?}",
                        path
                    ))?;
                // scan the dependencies of the path dependency
                stack.push(dep_package)
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
    wheel_dir: impl AsRef<Path>,
    metadata21: &Metadata21,
    manifest_path: impl AsRef<Path>,
    cargo_metadata: &Metadata,
    sdist_include: Option<&Vec<String>>,
) -> Result<PathBuf> {
    let path_deps = find_path_deps(cargo_metadata)?;

    let mut writer = SDistWriter::new(wheel_dir, &metadata21)?;
    let root_dir = PathBuf::from(format!(
        "{}-{}",
        &metadata21.get_distribution_escaped(),
        &metadata21.get_version_escaped()
    ));

    // Add local path dependencies
    for (name, path) in path_deps.iter() {
        add_crate_to_source_distribution(
            &mut writer,
            &PathBuf::from(path).join("Cargo.toml"),
            &root_dir.join(LOCAL_DEPENDENCIES_FOLDER).join(name),
            &path_deps,
            false,
            true,
        )
        .context(format!(
            "Failed to add local dependency {} at {} to the source distribution",
            name,
            path.to_string_lossy()
        ))?;
    }

    // Add the main crate
    add_crate_to_source_distribution(
        &mut writer,
        &manifest_path,
        &root_dir,
        &path_deps,
        false,
        false,
    )?;

    let manifest_dir = manifest_path.as_ref().parent().unwrap();

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

/// The `[build-system]` section of a pyproject.toml as specified in PEP 517
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct BuildSystem {
    requires: Vec<String>,
    build_backend: String,
}

/// The `[tool]` section of a pyproject.toml
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Tool {
    maturin: Option<ToolMaturin>,
}

/// The `[tool.maturin]` section of a pyproject.toml
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct ToolMaturin {
    sdist_include: Option<Vec<String>>,
}

/// A pyproject.toml as specified in PEP 517
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PyProjectToml {
    build_system: BuildSystem,
    tool: Option<Tool>,
}

impl PyProjectToml {
    pub fn sdist_include(&self) -> Option<&Vec<String>> {
        self.tool.as_ref()?.maturin.as_ref()?.sdist_include.as_ref()
    }
}

/// Returns the contents of a pyproject.toml with a `[build-system]` entry or an error
///
/// Does no specific error handling because it's only used to check whether or not to build
/// source distributions
pub fn get_pyproject_toml(project_root: impl AsRef<Path>) -> Result<PyProjectToml> {
    let path = project_root.as_ref().join("pyproject.toml");
    let contents = fs::read_to_string(&path).context(format!(
        "Couldn't find pyproject.toml at {}",
        path.display()
    ))?;
    let cargo_toml = toml::from_str(&contents)
        .map_err(|err| format_err!("pyproject.toml is not PEP 517 compliant: {}", err))?;
    Ok(cargo_toml)
}
