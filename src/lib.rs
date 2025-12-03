//! Builds wheels from a crate that exposes python bindings through pyo3
//!
//! The high-level api is [BuildOptions], which can be converted into the [BuildContext], which
//! then uses [compile()] and builds the appropriate wheels.
//!
//! # Cargo features
//!
//! Default features: full, rustls
//!
//! - full: Enables the cli-completion, cross-compile, and scaffolding features
//!
//! - cli-completion: Enables the generation of shell completion for the maturin CLI
//!
//! - cross-compile: Enables cross compiling using either zig or xwin
//!
//! - scaffolding: Enables the 'maturin new' and 'maturin generate-ci' commands
//!
//! - schemars: Enables the `maturin generate-json-schema` to generate a JSON schema
//!   for the `tool.maturin` section of the pyproject.toml file
//!
//! - static: Builds maturin with statically linked dependencies
//!
//! - rustls: Makes dependencies use the rustls stack so that we can build maturin in a
//!   CentOS 6 docker container and which makes maturin itself manylinux compliant.
//!
//! - native-tls: Makes dependencies use the platform native tls stack
//!
//! Deprecate features:
//!
//! The following features no longer configure anything within maturin but remain in order
//! to preserve backwards compatibility.
//!
//! - human-panic
//! - log
//! - password-storage
//! - upload

#![deny(missing_docs)]

pub use crate::bridge::{Abi3Version, BridgeModel, PyO3, PyO3Crate};
pub use crate::build_context::{BuildContext, BuiltWheelMetadata};
pub use crate::build_options::{BuildOptions, CargoOptions, TargetTriple};
pub use crate::cargo_toml::CargoToml;
pub use crate::compile::{BuildArtifact, compile};
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
pub use crate::source_distribution::find_path_deps;
pub use auditwheel::PlatformTag;
pub use target::Target;

mod archive_source;
mod auditwheel;
mod binding_generator;
mod bridge;
mod build_context;
mod build_options;
mod cargo_toml;
#[cfg(feature = "scaffolding")]
/// Generate CI configuration
pub mod ci;
mod compile;
mod compression;
mod cross_compile;
mod develop;
mod generate_json_schema;
mod metadata;
mod module_writer;
#[cfg(feature = "scaffolding")]
mod new_project;
mod project_layout;
pub mod pyproject_toml;
mod python_interpreter;
mod source_distribution;
mod target;
