mod builder;
mod repair;
mod wheels;

pub use builder::BuildContextBuilder;

use crate::auditwheel::AuditWheelMode;
use crate::auditwheel::{PlatformTag, Policy};
use crate::bridge::Abi3Version;
use crate::build_options::CargoOptions;
use crate::compile::CompileTarget;
use crate::compression::CompressionOptions;
use crate::module_writer::{WheelWriter, write_pth};
use crate::project_layout::ProjectLayout;
use crate::pyproject_toml::ConditionalFeature;
use crate::sbom::generate_sbom_data;
use crate::source_distribution::source_distribution;
use crate::target::validate_wheel_filename_for_pypi;
use crate::{
    BridgeModel, Metadata24, PyProjectToml, PythonInterpreter, Target, VirtualWriter,
    pyproject_toml::Format, pyproject_toml::SbomConfig,
};
use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::Metadata;
use fs_err as fs;
use ignore::overrides::{Override, OverrideBuilder};
use normpath::PathExt;
use std::path::PathBuf;
use tracing::instrument;

/// Contains all the metadata required to build the crate
#[derive(Clone)]
pub struct BuildContext {
    /// The platform, i.e. os and pointer width
    pub target: Target,
    /// List of Cargo targets to compile
    pub compile_targets: Vec<CompileTarget>,
    /// Whether this project is pure rust or rust mixed with python
    pub project_layout: ProjectLayout,
    /// The path to pyproject.toml. Required for the source distribution
    pub pyproject_toml_path: PathBuf,
    /// Parsed pyproject.toml if any
    pub pyproject_toml: Option<PyProjectToml>,
    /// Python Package Metadata 2.3
    pub metadata24: Metadata24,
    /// The name of the crate
    pub crate_name: String,
    /// The name of the module can be distinct from the package name, mostly
    /// because package names normally contain minuses while module names
    /// have underscores. The package name is part of metadata24
    pub module_name: String,
    /// The path to the Cargo.toml. Required for the cargo invocations
    pub manifest_path: PathBuf,
    /// Directory for all generated artifacts
    pub target_dir: PathBuf,
    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    pub out: PathBuf,
    /// Strip the library for minimum file size
    pub strip: bool,
    /// Checking the linked libraries for manylinux/musllinux compliance
    pub auditwheel: AuditWheelMode,
    /// When compiling for manylinux, use zig as linker to ensure glibc version compliance
    #[cfg(feature = "zig")]
    pub zig: bool,
    /// Whether to use the manylinux/musllinux or use the native linux tag (off)
    pub platform_tag: Vec<PlatformTag>,
    /// The available python interpreter
    pub interpreter: Vec<PythonInterpreter>,
    /// Cargo.toml as resolved by [cargo_metadata]
    pub cargo_metadata: Metadata,
    /// Whether to use universal2 or use the native macOS tag (off)
    pub universal2: bool,
    /// Build editable wheels
    pub editable: bool,
    /// Cargo build options
    pub cargo_options: CargoOptions,
    /// Compression options
    pub compression: CompressionOptions,
    /// Whether to validate wheels against PyPI platform tag rules
    pub pypi_validation: bool,
    /// SBOM configuration
    pub sbom: Option<SbomConfig>,
    /// Include the import library (.dll.lib) in the wheel on Windows
    pub include_import_lib: bool,
    /// Include debug info files (.pdb, .dSYM, .dwp) in the wheel
    pub include_debuginfo: bool,
    /// Cargo features conditionally enabled based on the target Python version/implementation
    pub conditional_features: Vec<ConditionalFeature>,
}

/// The wheel file location and its Python version tag (e.g. `py3`).
///
/// For bindings the version tag contains the Python interpreter version
/// they bind against (e.g. `cp37`).
pub type BuiltWheelMetadata = (PathBuf, String);

impl BuildContext {
    /// Checks which kind of bindings we have (pyo3/rust-cypthon or cffi or bin) and calls the
    /// correct builder.
    #[instrument(skip_all)]
    pub fn build_wheels(&self) -> Result<Vec<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the wheels")?;

        // Generate SBOM data once for all wheels (the Rust dependency graph
        // is the same regardless of the target Python interpreter).
        let sbom_data = generate_sbom_data(self)?;

        let wheels = match self.bridge() {
            BridgeModel::Bin(None) => self.build_bin_wheel(None, &sbom_data)?,
            BridgeModel::Bin(Some(..)) => self.build_bin_wheels(&self.interpreter, &sbom_data)?,
            BridgeModel::PyO3(crate::PyO3 { abi3, .. }) => match abi3 {
                Some(Abi3Version::Version(major, minor)) => {
                    self.build_abi3_wheels(Some((*major, *minor)), &sbom_data)?
                }
                Some(Abi3Version::CurrentPython) => self.build_abi3_wheels(None, &sbom_data)?,
                None => self.build_pyo3_wheels(&self.interpreter, &sbom_data)?,
            },
            BridgeModel::Cffi => self.build_cffi_wheel(&sbom_data)?,
            BridgeModel::UniFfi => self.build_uniffi_wheel(&sbom_data)?,
        };

        // Validate wheel filenames against PyPI platform tag rules if requested
        if self.pypi_validation {
            for wheel in &wheels {
                let filename = wheel
                    .0
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| anyhow!("Invalid wheel filename: {:?}", wheel.0))?;

                if let Err(error) = validate_wheel_filename_for_pypi(filename) {
                    bail!("PyPI validation failed: {}", error);
                }
            }
        }

        Ok(wheels)
    }

    /// Bridge model
    pub fn bridge(&self) -> &BridgeModel {
        // FIXME: currently we only allow multiple bin targets so bridges are all the same
        &self.compile_targets[0].bridge_model
    }

    /// Builds a source distribution and returns the same metadata as [BuildContext::build_wheels]
    pub fn build_source_distribution(&self) -> Result<Option<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the source distribution")?;

        match self.pyproject_toml.as_ref() {
            Some(pyproject) => {
                let sdist_path =
                    source_distribution(self, pyproject, self.excludes(Format::Sdist)?)
                        .context("Failed to build source distribution")?;
                Ok(Some((sdist_path, "source".to_string())))
            }
            None => Ok(None),
        }
    }

    /// Return the tags of the wheel that this build context builds.
    pub fn tags_from_bridge(&self) -> Result<Vec<String>> {
        let tags = match self.bridge() {
            BridgeModel::PyO3(bindings) | BridgeModel::Bin(Some(bindings)) => match bindings.abi3 {
                Some(Abi3Version::Version(major, minor)) => {
                    let platform = self.get_platform_tag(&[PlatformTag::Linux])?;
                    vec![format!("cp{major}{minor}-abi3-{platform}")]
                }
                Some(Abi3Version::CurrentPython) => {
                    let interp = &self.interpreter[0];
                    let platform = self.get_platform_tag(&[PlatformTag::Linux])?;
                    vec![format!(
                        "cp{major}{minor}-abi3-{platform}",
                        major = interp.major,
                        minor = interp.minor
                    )]
                }
                None => {
                    vec![self.interpreter[0].get_tag(self, &[PlatformTag::Linux])?]
                }
            },
            BridgeModel::Bin(None) | BridgeModel::Cffi | BridgeModel::UniFfi => {
                vec![self.get_universal_tag(&[PlatformTag::Linux])?]
            }
        };
        Ok(tags)
    }

    fn add_pth(&self, writer: &mut VirtualWriter<WheelWriter>) -> Result<()> {
        if self.editable {
            write_pth(writer, &self.project_layout, &self.metadata24)?;
        }
        Ok(())
    }

    fn excludes(&self, format: Format) -> Result<Override> {
        let project_dir = match self.pyproject_toml_path.normalize() {
            Ok(pyproject_toml_path) => pyproject_toml_path.into_path_buf(),
            Err(_) => self.manifest_path.normalize()?.into_path_buf(),
        };
        let mut excludes = OverrideBuilder::new(project_dir.parent().unwrap());
        if let Some(pyproject) = self.pyproject_toml.as_ref()
            && let Some(glob_patterns) = &pyproject.exclude()
        {
            for glob in glob_patterns
                .iter()
                .filter_map(|glob_pattern| glob_pattern.targets(format))
            {
                excludes.add(glob)?;
            }
        }
        // Ignore sdist output files so that we don't include them in the sdist
        if matches!(format, Format::Sdist) {
            let glob_pattern = format!(
                "{}{}{}-*.tar.gz",
                self.out.display(),
                std::path::MAIN_SEPARATOR,
                &self.metadata24.get_distribution_escaped(),
            );
            excludes.add(&glob_pattern)?;
        }
        Ok(excludes.build()?)
    }

    /// Returns the platform part of the tag for the wheel name
    pub fn get_platform_tag(&self, platform_tags: &[PlatformTag]) -> Result<String> {
        crate::target::get_platform_tag(
            &self.target,
            platform_tags,
            self.universal2,
            self.pyproject_toml.as_ref(),
            &self.manifest_path,
        )
    }

    /// Returns the platform tag without python version (e.g. `py3-none-manylinux_2_17_x86_64`)
    pub fn get_universal_tag(&self, platform_tags: &[PlatformTag]) -> Result<String> {
        let platform = self.get_platform_tag(platform_tags)?;
        Ok(format!("py3-none-{platform}"))
    }

    /// Returns user-specified platform tags, or falls back to the auditwheel
    /// policy tag when no explicit tags were provided.
    fn resolve_platform_tags(&self, policy: &Policy) -> Vec<PlatformTag> {
        if self.platform_tag.is_empty() {
            vec![policy.platform_tag()]
        } else {
            self.platform_tag.clone()
        }
    }
}
