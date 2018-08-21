//! The sdist format is (mostly) specified in PEP 517

use failure::Error;
use failure::ResultExt;
use libflate::gzip::Encoder;
use metadata::WheelMetadata;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str;
use tar;
use toml;
use BuildContext;

/// A pyproject.toml file
#[derive(Serialize, Deserialize)]
pub(crate) struct Pyproject {
    pub(crate) tool: PyprojectTool,
    #[serde(rename = "build-system")]
    pub(crate) build_system: PyprojectBuildSystem,
}

/// The [requires] section in a pyproject.toml
///
/// Specified in PEP 517
#[derive(Serialize, Deserialize)]
pub(crate) struct PyprojectBuildSystem {
    pub(crate) requires: Vec<String>,
    #[serde(rename = "build-backend")]
    pub(crate) build_backend: String,
}

/// The [tool] section in a pyproject.toml file
#[derive(Serialize, Deserialize)]
pub(crate) struct PyprojectTool {
    #[serde(rename = "pyo3-pack")]
    pub(crate) pyo3_pack: PyprojectToolPyo3Pack,
}

/// The [tool.pyo3-pack] section in a project.toml file
#[derive(Serialize, Deserialize)]
pub(crate) struct PyprojectToolPyo3Pack {
    pub(crate) build_context: BuildContext,
    pub(crate) scripts: HashMap<String, String>,
}

/// Builds a source distribution (sdist) for a package
///
/// Besides the files that are selected `cargo package`, the source
/// distribution will include a pyproject.toml, which contains the build
/// instructions, and a PKG-INFO file, which has the same content as METADATA
/// for wheels. See build_sdist in PEP 517.
///
/// We want to include the same files in the source distribution as cargo
/// includes in a published package, i.e. combine .gitignore, package.include
/// and package.exclude. See https://docs.rs/cargo/0.28.0/cargo/sources/path/struct.PathSource.html#method.list_files
/// for Cargo's algorithm
pub fn build_source_distribution(
    build_context: &BuildContext,
    metadata: &WheelMetadata,
    target_file: &Path,
) -> Result<(), Error> {
    let output = Command::new("cargo")
        .args(&["package", "--list", "--quiet"])
        .output()
        .context("Failed to get a list of files for the source distribution from cargo")?;

    let file = File::create(target_file).context(format!(
        "Failed to create source distribution at {}",
        target_file.display()
    ))?;
    // We need some normal permission on file for the in memory files since tar
    // otherwise does weird stuff
    let normal_metadata = file.metadata()?;

    let encoder = Encoder::new(file)?;
    let mut archive = tar::Builder::new(encoder);

    let folder = PathBuf::from(format!(
        "{}-{}",
        metadata.metadata21.name, metadata.metadata21.version
    ));

    // Add all the files from the cargo package to the zip
    for filename in str::from_utf8(&output.stdout).unwrap().lines() {
        let mut f = File::open(filename).context("Can't open file advertised by cargo")?;
        archive.append_file(folder.join(filename), &mut f).unwrap();
    }

    // Add the pyproject.toml
    let pyproject = Pyproject {
        tool: PyprojectTool {
            pyo3_pack: PyprojectToolPyo3Pack {
                build_context: build_context.clone(),
                scripts: metadata.scripts.clone(),
            },
        },
        build_system: PyprojectBuildSystem {
            requires: vec![env!("CARGO_PKG_NAME").to_string()],
            build_backend: "pyo3_pack:install_sdist".to_string(),
        },
    };

    let pyproject_toml = toml::to_string_pretty(&pyproject)?;
    let mut header = tar::Header::new_gnu();
    header.set_metadata(&normal_metadata);
    header.set_size(pyproject_toml.as_bytes().len() as u64);
    header.set_cksum();
    archive.append_data(
        &mut header,
        folder.join("pyproject.toml"),
        pyproject_toml.as_bytes(),
    )?;

    let pkg_info = metadata.metadata21.to_file_contents();
    let mut header = tar::Header::new_gnu();
    header.set_metadata(&normal_metadata);
    header.set_size(pkg_info.as_bytes().len() as u64);
    header.set_cksum();
    archive.append_data(&mut header, folder.join("PKG-INFO"), pkg_info.as_bytes())?;

    archive.into_inner()?.finish().into_result()?;

    Ok(())
}
