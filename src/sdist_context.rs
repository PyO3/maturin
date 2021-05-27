use crate::{source_distribution, CargoToml, Metadata21, PyProjectToml};
use anyhow::{Context, Result};
use cargo_metadata::{Metadata, MetadataCommand};
use fs_err as fs;
use std::path::PathBuf;

/// Contains all the metadata required to build the source distribution
#[derive(Clone)]
pub struct SDistContext {
    /// Python Package Metadata 2.1
    pub metadata21: Metadata21,
    /// The name of the module can be distinct from the package name, mostly
    /// because package names normally contain minuses while module names
    /// have underscores. The package name is part of metadata21
    pub module_name: String,
    /// The directory where Cargo.toml is located
    pub manifest_dir: PathBuf,
    /// The path to the Cargo.toml. Required for the cargo invocations
    pub manifest_path: PathBuf,
    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    pub out: PathBuf,
    /// Cargo.toml as resolved by [cargo_metadata]
    pub cargo_metadata: Metadata,
}

impl SDistContext {
    /// Creates a new source distribution build context from the project
    /// `Cargo.toml` path and the tarball output directory.
    ///
    /// Tries to fill the missing metadata by parsing `Cargo.toml` and
    /// querying `cargo metadata`.
    pub fn new(manifest_path: PathBuf, out: Option<PathBuf>) -> Result<Self> {
        let manifest_dir = manifest_path.parent().unwrap().to_path_buf();

        let cargo_toml = CargoToml::from_path(&manifest_path)?;

        let metadata21 = Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir)
            .context("Failed to parse Cargo.toml into python metadata")?;

        let crate_name = &cargo_toml.package.name;

        // If the package name contains minuses, you must declare a module with
        // underscores as lib name
        let module_name = cargo_toml
            .lib
            .as_ref()
            .and_then(|lib| lib.name.as_ref())
            .unwrap_or(&crate_name)
            .to_owned();

        let cargo_metadata = MetadataCommand::new()
            .manifest_path(&manifest_path)
            .exec()
            .context("Cargo metadata failed. Do you have cargo in your PATH?")?;

        let wheel_dir = match out {
            Some(ref dir) => dir.clone(),
            None => PathBuf::from(&cargo_metadata.target_directory).join("wheels"),
        };

        Ok(SDistContext {
            metadata21,
            module_name,
            manifest_dir,
            manifest_path,
            out: wheel_dir,
            cargo_metadata,
        })
    }

    /// Builds a source distribution and returns the source tarball file path.
    pub fn build_source_distribution(&self) -> Result<PathBuf> {
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the source distribution")?;

        // Ensure the project has a compliant pyproject.toml
        let pyproject = PyProjectToml::new(&self.manifest_dir)
            .context("A pyproject.toml with a PEP 517 compliant `[build-system]` table is required to build a source distribution")?;

        source_distribution(
            &self.out,
            &self.metadata21,
            &self.manifest_path,
            &self.cargo_metadata,
            pyproject.sdist_include(),
        )
        .context("Failed to build source distribution")
    }
}
