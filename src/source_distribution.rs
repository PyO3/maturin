use crate::module_writer::ModuleWriter;
use crate::{Metadata21, SDistWriter};
use failure::{bail, Error, ResultExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str;

/// Creates a source distribution
///
/// Runs `cargo package --list --allow-dirty` to obtain a list of files to package.
///
/// The source distribution format is specified in
/// [PEP 517 under "build_sdist"](https://www.python.org/dev/peps/pep-0517/#build-sdist)
pub fn source_distribution(
    wheel_dir: impl AsRef<Path>,
    metadata21: &Metadata21,
    manifest_dir: impl AsRef<Path>,
) -> Result<PathBuf, Error> {
    let output = Command::new("cargo")
        .args(&["package", "--list", "--allow-dirty", "--manifest-path"])
        .arg(manifest_dir.as_ref())
        .stderr(Stdio::inherit())
        .output()
        .context("Failed to run cargo")?;
    if !output.status.success() {
        bail!(
            "Failed to query file list from cargo: {}\n--- Stdout:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
        );
    }

    let file_list: Vec<&Path> = str::from_utf8(&output.stdout)
        .context("Cargo printed invalid utf-8 ಠ_ಠ")?
        .lines()
        .map(Path::new)
        .collect();

    let mut writer = SDistWriter::new(wheel_dir, &metadata21)?;
    for relative_to_cwd in file_list {
        let relative_to_project_root = relative_to_cwd
            .strip_prefix(manifest_dir.as_ref().parent().unwrap())
            .context("Cargo returned an out-of-tree path ಠ_ಠ")?;
        writer.add_file(relative_to_project_root, relative_to_cwd)?;
    }

    writer.add_bytes("PKG-INFO", metadata21.to_file_contents().as_bytes())?;

    let source_distribution_path = writer.finish()?;

    println!(
        "📦 Built source distribution to {}",
        source_distribution_path.display()
    );

    Ok(source_distribution_path)
}
