//! Builds wheels from a crate that exposes python bindings through pyo3
//!
//! The high-level api is [BuildOptions], which can be converted into the [BuildContext], which
//! then uses [compile()] and builds the appropriate wheels.
//!
//! # Cargo features
//!
//! Default features: full, rustls
//!
//! - full: Bundles cli-completion, cross-compile, scaffolding, upload, sbom and auditwheel.
//!
//! - upload: Uses ureq to add the upload and publish commands.
//!
//! - rustls: Makes ureq (and cargo-xwin) use the rustls stack so that we can build maturin in
//!   a CentOS 6 docker container and which maturin itself manylinux compliant.
//!
//! - native-tls: Makes ureq (and cargo-xwin) use the platform-native tls stack.
//!
//! - sbom: Generates a CycloneDX software bill of materials for the built wheel.
//!
//! - auditwheel: Enables dependency auditing and wheel repair for macOS (install-name/rpath
//!   rewriting) and Windows (PE import patching and DLL bundling); Linux auditing and
//!   patchelf-based repair work without it. Includes pure-Rust ad-hoc code signing for macOS
//!   wheels when cross-compiling from a non-macOS host (native builds use Apple's codesign).
//!
//! - scaffolding: Enables the `maturin new`/`init`/`generate-ci` project scaffolding commands.
//!
//! - cross-compile: Enables cross compilation support via zig (cargo-zigbuild) and xwin. The
//!   zig and xwin sub-features can also be enabled individually.
//!
//! - cli-completion: Enables shell completion generation for the CLI.
//!
//! - schemars: Enables generating the JSON schema for maturin's configuration.
//!
//! - static (off by default): Statically links liblzma via xz2/static.
//!
//! - password-storage (off by default, implies upload): Uses the keyring package to store the
//!   password. maturin only enables keyring's native macOS/Windows/Linux backends, so there is
//!   no BSD backend; most builds on the github releases page enable this.
//!
//! - log, human-panic (deprecated): No longer do anything and are kept only for compatibility.

#![deny(missing_docs)]

pub use crate::bridge::{BridgeModel, PyO3, PyO3Crate, StableAbi, StableAbiKind, StableAbiVersion};
pub use crate::build_context::{
    ArtifactContext, BuildContext, BuiltArtifactTag, BuiltWheel, ProjectContext, PythonContext,
};
pub use crate::build_options::{BuildOptions, OutputOptions, PlatformOptions, PythonOptions};
pub use crate::build_orchestrator::BuildOrchestrator;
pub use crate::cargo_options::{CargoOptions, TargetTriple};
pub use crate::cargo_toml::CargoToml;
pub use crate::compile::{BuildArtifact, CompileResult, ThinArtifact, compile};
pub use crate::compression::{CompressionMethod, CompressionOptions};
pub use crate::develop::{DevelopOptions, develop};
#[cfg(feature = "schemars")]
pub use crate::generate_json_schema::{GenerateJsonSchemaOptions, Mode, generate_json_schema};
pub use crate::metadata::{Metadata24, WheelMetadata};
pub use crate::module_writer::{
    ModuleWriter, PathWriter, SDistWriter, VirtualWriter, WheelWriter, write_dist_info,
};
#[cfg(feature = "scaffolding")]
pub use crate::new_project::{GenerateProjectOptions, init_project, new_project};
pub use crate::pyproject_toml::PyProjectToml;
pub use crate::python_interpreter::PythonInterpreter;
pub use crate::source_distribution::{UnpackedSdist, find_path_deps, unpack_sdist};
#[cfg(feature = "upload")]
pub use crate::upload::{PublishOpt, Registry, UploadError, upload, upload_ui};
pub use auditwheel::{CompatibilityTag, PlatformTag};
pub use target::{Target, WheelTag};

mod archive_source;
mod auditwheel;
mod binding_generator;
mod bridge;
mod build_context;
mod build_options;
mod build_orchestrator;
/// Cargo build options
pub mod cargo_options;
mod cargo_toml;
#[cfg(feature = "scaffolding")]
/// Generate CI configuration
pub mod ci;
mod compile;
mod compression;
mod cross_compile;
pub(crate) mod develop;
mod generate_json_schema;
mod metadata;
mod module_writer;
#[cfg(feature = "scaffolding")]
mod new_project;
/// Profile-Guided Optimization (PGO) orchestration
pub(crate) mod pgo;
mod project_layout;
pub mod pyproject_toml;
mod python_interpreter;
mod sbom;
mod source_distribution;
mod target;
#[cfg(test)]
mod test_utils;
pub(crate) mod ui;
#[cfg(feature = "upload")]
mod upload;
pub(crate) mod util;
