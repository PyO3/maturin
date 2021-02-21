mod policy;

use crate::Manylinux;
use crate::Target;
use anyhow::Result;
use fs_err::File;
use goblin::elf::Elf;
use regex::Regex;
use std::io;
use std::io::Read;
use std::path::Path;
use thiserror::Error;

use policy::POLICIES;

/// Error raised during auditing an elf file for manylinux compatibility
#[derive(Error, Debug)]
#[error("Ensuring manylinux compliance failed")]
pub enum AuditWheelError {
    /// The wheel couldn't be read
    #[error("Failed to read the wheel")]
    IOError(#[source] io::Error),
    /// Reexports elfkit parsing errors
    #[error("Goblin failed to parse the elf file")]
    GoblinError(#[source] goblin::error::Error),
    /// The elf file isn't manylinux compatible. Contains the list of offending
    /// libraries.
    #[error(
        "Your library links libpython ({0}), which libraries must not do. Have you forgotten to activate the extension-module feature?",
    )]
    LinksLibPythonError(String),
    /// The elf file isn't manylinux compatible. Contains the list of offending
    /// libraries.
    #[error(
        "Your library is not manylinux compliant because it links the following forbidden libraries: {0:?}",
    )]
    ManylinuxValidationError(Vec<String>),
}

/// An (incomplete) reimplementation of auditwheel, which checks elf files for
/// manylinux compliance. Returns an error for non compliant elf files
///
/// Only checks for the libraries marked as NEEDED, but not for symbol versions
/// (e.g. requiring a too recent glibc isn't caught).
pub fn auditwheel_rs(
    path: &Path,
    target: &Target,
    manylinux: &Manylinux,
) -> Result<(), AuditWheelError> {
    if !target.is_linux() || matches!(manylinux, Manylinux::Off) {
        return Ok(());
    }
    let reference = POLICIES
        .iter()
        .find(|p| p.name == manylinux.to_string())
        .map(|p| &p.lib_whitelist)
        .unwrap();
    let mut file = File::open(path).map_err(AuditWheelError::IOError)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(AuditWheelError::IOError)?;
    let elf = Elf::parse(&buffer).map_err(AuditWheelError::GoblinError)?;
    // This returns essentially the same as ldd
    let deps: Vec<String> = elf.libraries.iter().map(ToString::to_string).collect();

    let mut offenders = Vec::new();
    for dep in deps {
        // Skip dynamic linker/loader
        if dep.starts_with("ld-linux") || dep == "ld64.so.2" || dep == "ld64.so.1" {
            continue;
        }
        if !reference.contains(&dep) {
            offenders.push(dep);
        }
    }

    // Checks if we can give a more helpful error message
    let is_libpython = Regex::new(r"^libpython3\.\d+\.so\.\d+\.\d+$").unwrap();
    match offenders.as_slice() {
        [] => Ok(()),
        [lib] if is_libpython.is_match(lib) => {
            Err(AuditWheelError::LinksLibPythonError(lib.clone()))
        }
        offenders => Err(AuditWheelError::ManylinuxValidationError(
            offenders.to_vec(),
        )),
    }
}
