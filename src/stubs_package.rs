use crate::{Metadata21, ModuleWriter, WheelWriter};
use anyhow::{Context, Result};
use fs_err as fs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Creates the type information stubs package if the type information
/// source file exists.
pub fn stubs_package(
    wheel_dir: impl AsRef<Path>,
    module_name: &str,
    metadata21: &Metadata21,
) -> Result<Option<(PathBuf, Metadata21)>> {
    let mut stubs_source_path: PathBuf = module_name.into();
    stubs_source_path.set_extension("pyi");

    if !stubs_source_path.exists() {
        return Ok(None);
    }

    let mut stubs_metadata21 = metadata21.clone();

    // Build PEP 561 compliant stubs package name
    stubs_metadata21.name += "-stubs";
    // Module name may differ from the package name
    let mut stubs_module_name = module_name.to_string();
    stubs_module_name += "-stubs";

    let scripts = HashMap::new();
    let tags = ["py3-none-any".to_string()];

    fs::create_dir_all(&wheel_dir)
        .context("Failed to create the target directory for the stubs package")?;

    let mut writer = WheelWriter::new(
        "py3-none-any",
        wheel_dir.as_ref(),
        &stubs_metadata21,
        &scripts,
        &tags,
    )
    .context("Failed to create the type information stubs package wheel file")?;

    let mut stubs_package_initpy: PathBuf = stubs_module_name.into();
    stubs_package_initpy.push("__init__.pyi");

    writer.add_file(&stubs_package_initpy, &stubs_source_path)?;

    let stubs_package_path = writer.finish()?;

    println!(
        "📦 Built type information stubs package to {}",
        stubs_package_path.display()
    );

    Ok(Some((stubs_package_path, stubs_metadata21)))
}
