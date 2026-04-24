mod builder;
mod repair;

pub use builder::BuildContextBuilder;
pub(crate) use repair::link_or_copy;

use crate::auditwheel::AuditWheelMode;
use crate::auditwheel::PlatformTag;
use crate::cargo_options::CargoOptions;
use crate::compile::CompileTarget;
use crate::compression::CompressionOptions;
use crate::pgo::PgoPhase;
use crate::project_layout::ProjectLayout;
use crate::pyproject_toml::ConditionalFeature;
use crate::{
    BridgeModel, Metadata24, PyProjectToml, PythonInterpreter, Target, pyproject_toml::SbomConfig,
};
use anyhow::Result;
use cargo_metadata::Metadata;
use std::path::PathBuf;

/// The input part of the build context.
///
/// Contains static information about the project being built, such as its
/// filesystem layout, manifest paths, and resolved metadata. This information
/// is generally independent of the target Python interpreter.
#[derive(Clone, Debug)]
pub struct ProjectContext {
    /// The platform, i.e. os and pointer width
    pub target: Target,
    /// Whether this project is pure rust or rust mixed with python
    pub project_layout: ProjectLayout,
    /// The path to pyproject.toml. Required for the source distribution
    pub pyproject_toml_path: PathBuf,
    /// Parsed pyproject.toml if any
    pub pyproject_toml: Option<PyProjectToml>,
    /// Python Package Metadata 2.4
    pub metadata24: Metadata24,
    /// The name of the crate
    pub crate_name: String,
    /// The name of the module
    pub module_name: String,
    /// The path to the Cargo.toml. Required for the cargo invocations
    pub manifest_path: PathBuf,
    /// Directory for all generated artifacts
    pub target_dir: PathBuf,
    /// Cargo.toml as resolved by [cargo_metadata]
    pub cargo_metadata: Metadata,
    /// Whether to use universal2 or use the native macOS tag (off)
    pub universal2: bool,
    /// Build editable wheels
    pub editable: bool,
    /// Cargo build options
    pub cargo_options: CargoOptions,
    /// Cargo features conditionally enabled based on the target Python version/implementation
    pub conditional_features: Vec<ConditionalFeature>,
    /// List of Cargo targets to compile
    pub compile_targets: Vec<CompileTarget>,
}

impl ProjectContext {
    /// Bridge model
    pub fn bridge(&self) -> &BridgeModel {
        // FIXME: currently we only allow multiple bin targets so bridges are all the same
        &self.compile_targets[0].bridge_model
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
}

/// The output part of the build context.
///
/// Manages configuration for the final artifacts produced by the build,
/// such as output directories, symbol stripping, compression settings,
/// and SBOM generation.
#[derive(Clone, Debug)]
pub struct ArtifactContext {
    /// The directory to store the built wheels in
    pub out: PathBuf,
    /// Strip the library for minimum file size
    pub strip: bool,
    /// Compression options
    pub compression: CompressionOptions,
    /// SBOM configuration
    pub sbom: Option<SbomConfig>,
    /// Include the import library in the wheel on Windows
    pub include_import_lib: bool,
    /// Include debug info files (.pdb, .dSYM, .dwp) in the wheel
    pub include_debuginfo: bool,
    /// Current PGO build phase (if PGO is enabled)
    pub pgo_phase: Option<PgoPhase>,
    /// PGO training command from pyproject.toml (only set when --pgo is passed)
    pub pgo_command: Option<String>,
    /// Auto generate Python type stubs by introspecting the binary. Requires PyO3 and its "experimental-inspect" feature
    pub generate_stubs: bool,
}

/// The constraint part of the build context.
///
/// Defines the target environment where the built artifacts will run,
/// including the resolved Python interpreters, platform tags, and
/// compatibility requirements (e.g. auditwheel).
#[derive(Clone, Debug)]
pub struct PythonContext {
    /// Checking the linked libraries for manylinux compliance
    pub auditwheel: AuditWheelMode,
    /// When compiling for manylinux, use zig as linker
    #[cfg(feature = "zig")]
    pub zig: bool,
    /// Whether to use the manylinux/musllinux or use the native linux tag
    pub platform_tag: Vec<PlatformTag>,
    /// The available python interpreters
    pub interpreter: Vec<PythonInterpreter>,
    /// Whether to validate wheels against PyPI platform tag rules
    pub pypi_validation: bool,
}

/// The complete build context, partitioned into modular sub-contexts.
///
/// This structure reflects the build lifecycle:
/// **Input (Project) → Constraints (Python) → Output (Artifact).**
#[derive(Clone, Debug)]
pub struct BuildContext {
    /// Project context
    pub project: ProjectContext,
    /// Artifact context
    pub artifact: ArtifactContext,
    /// Python context
    pub python: PythonContext,
}

/// The wheel file location and its Python version tag (e.g. `py3`).
///
/// For bindings the version tag contains the Python interpreter version
/// they bind against (e.g. `cp37`).
pub type BuiltWheelMetadata = (PathBuf, String);
