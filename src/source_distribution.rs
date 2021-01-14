use crate::module_writer::ModuleWriter;
use crate::{Metadata21, SDistWriter};
use anyhow::{bail, format_err, Context, Result};
use cargo_metadata::Metadata;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{fs, str};

/// Checks if there a local/path dependencies which might not be included
/// when building the source distribution.
pub fn warn_on_local_deps(cargo_metadata: &Metadata) {
    let root_package = cargo_metadata
        .resolve
        .clone()
        .and_then(|y| y.root)
        .expect("Expected a resolve with a root");

    let local_deps: Vec<String> = cargo_metadata
        .packages
        .iter()
        .filter(|x| x.source.is_none())
        // Remove the package itself
        .filter(|x| x.id != root_package)
        .map(|x| x.name.clone())
        .collect();
    if !local_deps.is_empty() {
        eprintln!(
            "⚠ There are local dependencies, which the source distribution might not include: {}",
            local_deps.join(", ")
        );
    }
}

/// Creates a source distribution
///
/// Runs `cargo package --list --allow-dirty` to obtain a list of files to package.
///
/// The source distribution format is specified in
/// [PEP 517 under "build_sdist"](https://www.python.org/dev/peps/pep-0517/#build-sdist)
/// and in
/// https://packaging.python.org/specifications/source-distribution-format/#source-distribution-file-format
pub fn source_distribution(
    wheel_dir: impl AsRef<Path>,
    metadata21: &Metadata21,
    manifest_path: impl AsRef<Path>,
    sdist_include: Option<&Vec<String>>,
) -> Result<PathBuf> {
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
        .filter(|(target, _)| target != Path::new("Cargo.toml.orig"))
        .collect();

    if !target_source
        .iter()
        .any(|(target, _)| target == Path::new("pyproject.toml"))
    {
        bail!(
            "pyproject.toml was not included by `cargo package`. \
             Please make sure pyproject.toml is not excluded or build with `--no-sdist`"
        )
    }

    let mut writer = SDistWriter::new(wheel_dir, &metadata21)?;
    let root_dir = PathBuf::from(format!(
        "{}-{}",
        &metadata21.get_distribution_escaped(),
        &metadata21.get_version_escaped()
    ));
    writer.add_directory(&root_dir)?;
    for (target, source) in target_source {
        println!("{} {}", target.display(), source.display());
        writer.add_file(root_dir.join(target), source)?;
    }

    if let Some(include_targets) = sdist_include {
        for pattern in include_targets {
            println!("📦 Including files matching \"{}\"", pattern);
            for source in glob::glob(pattern)
                .expect("No files found for pattern")
                .filter_map(Result::ok)
            {
                writer.add_file(manifest_dir.join(&source).to_path_buf(), source)?;
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
