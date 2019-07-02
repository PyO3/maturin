//! Builds wheels from a crate that exposes python bindings through pyo3
//!
//! The high-level api is [BuildOptions], which can be converted into the [BuildContext], which
//! then uses [compile()] and builds the appropriate wheels.
//!
//! # Cargo features
//!
//! Default features: auditwheel, log, upload, rustls
//!
//! - auditwheel: Reimplements the more important part of the auditwheel
//! package in rust. A wheel is checked by default, unless deactivated by cli arguments
//!
//! - log: Configures pretty-env-logger, even though pyo3-pack doesn't use logging itself.
//!
//! - upload: Uses reqwest to add the upload command.
//!
//! - rustls: Makes reqwest use the rustls stack so that we can build pyo3-pack in a cent os 5
//! docker container and which pyo3-pack itself manylinux compliant.
//!
//! - password-storage (off by default): Uses the keyring package to store the password. keyring
//! pulls in a lot of shared libraries and outdated dependencies, so this is off by default, except
//! for the build on the github releases page.
//! (https://github.com/hwchen/secret-service-rs/issues/9)
//!
//! - human-panic (off by default): Adds human-panic, pulling in some outdated dependencies
//! (https://github.com/rust-clique/human-panic/pull/47)

#![deny(missing_docs)]

#[cfg(feature = "auditwheel")]
pub use crate::auditwheel::{auditwheel_rs, AuditWheelError};
pub use crate::build_context::BridgeModel;
pub use crate::build_context::BuildContext;
pub use crate::build_options::BuildOptions;
pub use crate::cargo_toml::CargoToml;
pub use crate::compile::compile;
pub use crate::develop::develop;
pub use crate::metadata::{Metadata21, WheelMetadata};
pub use crate::module_writer::{
    write_dist_info, ModuleWriter, PathWriter, SDistWriter, WheelWriter,
};
pub use crate::python_interpreter::PythonInterpreter;
pub use crate::target::{Manylinux, Target};
pub use source_distribution::{source_distribution, get_pyproject_toml};
#[cfg(feature = "upload")]
pub use {
    crate::registry::Registry,
    crate::upload::{upload, UploadError},
};

#[cfg(feature = "auditwheel")]
mod auditwheel;
mod build_context;
mod build_options;
mod cargo_toml;
mod compile;
mod develop;
mod metadata;
mod module_writer;
mod python_interpreter;
#[cfg(feature = "upload")]
mod registry;
mod source_distribution;
mod target;
#[cfg(feature = "upload")]
mod upload;
