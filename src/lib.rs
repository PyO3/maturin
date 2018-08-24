//! Builds wheels from a crate that exposes python bindings through pyo3
//!
//! The high-level api is [BuildContext], which internally calls [build_rust()]
//! and [build_wheel()].
//!
//! # Cargo features
//!
//! - auditwheel: Reimplements the more important part of the auditwheel
//! package in rust. This feature is enabled by default and means that every
//! wheel is check unless [skip_auditwheel](BuildContext.skip_auditwheel) is
//! set to true.
//!
//! - sdist: Allows creating sdist archives. Those archives can not be
//! installed yet, since (at least in the current 10.0.1) doesn't implement
//! PEP 517 and pyo3-pack doesn't implement the build backend api from that
//! PEP. It is therefore disabled by default. It also currently requires
//! nightly as it uses pyo3 for bindings and setting the crate type for lib to
//! rlib and cdylib.

#![deny(missing_docs)]
extern crate base64;
extern crate cargo_metadata;
#[cfg(feature = "auditwheel")]
extern crate elfkit;
extern crate regex;
#[macro_use]
extern crate failure;
extern crate atty;
extern crate reqwest;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate sha2;
#[macro_use]
extern crate structopt;
extern crate indicatif;
#[cfg(feature = "sdist")]
extern crate libflate;
#[cfg(feature = "sdist")]
extern crate tar;
extern crate target_info;
extern crate toml;
extern crate zip;

#[cfg(feature = "auditwheel")]
pub use auditwheel::{auditwheel_rs, AuditWheelError};
pub use build_context::BuildContext;
#[cfg(feature = "sdist")]
use capybara::prelude::*;
pub use cargo_toml::CargoToml;
pub use compile::compile;
pub use develop::develop;
pub use metadata::{Metadata21, WheelMetadata};
pub use python_interpreter::PythonInterpreter;
pub use registry::Registry;
#[cfg(feature = "sdist")]
pub use sdist::build_source_distribution;
pub use upload::{upload, upload_wheels, UploadError};
pub use wheel::build_wheel;

mod build_context;
mod compile;

#[cfg(feature = "auditwheel")]
mod auditwheel;
mod cargo_toml;
mod develop;
mod metadata;
mod python_interpreter;
mod registry;
#[cfg(feature = "sdist")]
mod sdist;
mod upload;
mod wheel;

#[cfg(feature = "sdist")]
capybara_init! {pyo3_pack, [], [install_sdist]}
