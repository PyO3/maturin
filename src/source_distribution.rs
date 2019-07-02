use crate::module_writer::ModuleWriter;
use crate::{Metadata21, SDistWriter};
use failure::{bail, Error, ResultExt};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{fs, str};

/// Creates a source distribution
///
/// Runs `cargo package --list --allow-dirty` to obtain a list of files to package.
///
/// The source distribution format is specified in
/// [PEP 517 under "build_sdist"](https://www.python.org/dev/peps/pep-0517/#build-sdist)
pub fn source_distribution(
    wheel_dir: impl AsRef<Path>,
    metadata21: &Metadata21,
    manifest_path: impl AsRef<Path>,
) -> Result<PathBuf, Error> {
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

    let target_source: Vec<(&Path, &Path)> = file_list
        .iter()
        .map(|relative_to_cwd| {
            let relative_to_project_root = relative_to_cwd
                .strip_prefix(manifest_path.as_ref().parent().unwrap())
                .unwrap_or(relative_to_cwd);
            (relative_to_project_root, *relative_to_cwd)
        })
        .collect();

    if !target_source
        .iter()
        .any(|(target, _)| target == &Path::new("pyproject.toml"))
    {
        bail!(
            "pyproject.toml was not included by `cargo package`. \
             Please make sure pyproject.toml is not excluded or build with `--no-sdist`"
        )
    }

    let mut writer = SDistWriter::new(wheel_dir, &metadata21)?;
    for (target, source) in target_source {
        writer.add_file(target, source)?;
    }

    writer.add_bytes("PKG-INFO", metadata21.to_file_contents().as_bytes())?;

    let source_distribution_path = writer.finish()?;

    println!(
        "📦 Built source distribution to {}",
        source_distribution_path.display()
    );

    Ok(source_distribution_path)
}

/// A pyproject.toml as specified in PEP 517
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PyProjectToml {
    build_system: toml::Value,
}

/// Returns the contents of a pyproject.toml with a `[build-system]` entry or an error
///
/// Does no specific error handling because it's only used to check whether or not to build
/// source distributions
pub fn get_pyproject_toml(project_root: impl AsRef<Path>) -> Result<PyProjectToml, Error> {
    let contents = fs::read_to_string(project_root.as_ref().join("pyproject.toml"))?;
    let cargo_toml = toml::from_str(&contents)?;
    Ok(cargo_toml)
}
