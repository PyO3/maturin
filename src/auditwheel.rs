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

/// As specified in "PEP 571 -- The manylinux2010 Platform Tag"
const MANYLINUX2010: &[&str] = &[
    "libgcc_s.so.1",
    "libstdc++.so.6",
    "libm.so.6",
    "libdl.so.2",
    "librt.so.1",
    "libcrypt.so.1",
    "libc.so.6",
    "libnsl.so.1",
    "libutil.so.1",
    "libpthread.so.0",
    "libresolv.so.2",
    "libX11.so.6",
    "libXext.so.6",
    "libXrender.so.1",
    "libICE.so.6",
    "libSM.so.6",
    "libGL.so.1",
    "libgobject-2.0.so.0",
    "libgthread-2.0.so.0",
    "libglib-2.0.so.0",
];

/// As specified in "PEP 599 -- The manylinux2014 Platform Tag"
const MANYLINUX2014: &[&str] = &[
    "libgcc_s.so.1",
    "libstdc++.so.6",
    "libm.so.6",
    "libdl.so.2",
    "librt.so.1",
    "libc.so.6",
    "libnsl.so.1",
    "libutil.so.1",
    "libpthread.so.0",
    "libresolv.so.2",
    "libX11.so.6",
    "libXext.so.6",
    "libXrender.so.1",
    "libICE.so.6",
    "libSM.so.6",
    "libGL.so.1",
    "libgobject-2.0.so.0",
    "libgthread-2.0.so.0",
    "libglib-2.0.so.0",
];

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
    if !target.is_linux() {
        return Ok(());
    }
    let reference: &[&str];
    match *manylinux {
        Manylinux::Manylinux2010 => reference = MANYLINUX2010,
        Manylinux::Manylinux2014 => reference = MANYLINUX2014,
        Manylinux::Off => return Ok(()),
    };
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
        if !reference.contains(&dep.as_str()) {
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
