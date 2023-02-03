//! Builds wheels from a crate that exposes python bindings through pyo3
//!
//! The high-level api is [BuildOptions], which can be converted into the [BuildContext], which
//! then uses [compile()] and builds the appropriate wheels.
//!
//! # Cargo features
//!
//! Default features: log, upload, rustls
//!
//! - log: Configures pretty-env-logger, even though maturin doesn't use logging itself.
//!
//! - upload: Uses ureq to add the upload command.
//!
//! - rustls: Makes ureq use the rustls stack so that we can build maturin in a CentOS 6
//! docker container and which maturin itself manylinux compliant.
//!
//! - native-tls: Makes ureq use the platform native tls stack
//!
//! - password-storage (off by default): Uses the keyring package to store the password. keyring
//! pulls in a lot of shared libraries and outdated dependencies, so this is off by default, except
//! for the build on the github releases page.
//! (https://github.com/hwchen/secret-service-rs/issues/9)

#![deny(missing_docs)]

pub use crate::build_context::{BridgeModel, BuildContext, BuiltWheelMetadata};
pub use crate::build_options::{BuildOptions, CargoOptions};
pub use crate::cargo_toml::CargoToml;
pub use crate::compile::{compile, BuildArtifact};
pub use crate::develop::develop;
pub use crate::metadata::{Metadata21, WheelMetadata};
pub use crate::module_writer::{
    write_dist_info, ModuleWriter, PathWriter, SDistWriter, WheelWriter,
};
#[cfg(feature = "scaffolding")]
pub use crate::new_project::{init_project, new_project, GenerateProjectOptions};
pub use crate::pyproject_toml::PyProjectToml;
pub use crate::python_interpreter::PythonInterpreter;
pub use crate::target::Target;
#[cfg(feature = "upload")]
pub use crate::upload::{upload, upload_ui, PublishOpt, Registry, UploadError};
pub use auditwheel::PlatformTag;

mod auditwheel;
mod build_context;
mod build_options;
mod cargo_toml;
#[cfg(feature = "scaffolding")]
/// Generate CI configuration
pub mod ci;
mod compile;
mod cross_compile;
mod develop;
mod metadata;
mod module_writer;
#[cfg(feature = "scaffolding")]
mod new_project;
mod project_layout;
pub mod pyproject_toml;
mod python_interpreter;
mod source_distribution;
mod target;
#[cfg(feature = "upload")]
mod upload;
