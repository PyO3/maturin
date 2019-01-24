use crate::Manylinux;
use crate::Target;
use failure::Fail;
use goblin;
use goblin::elf::Elf;
use std::fs::File;
use std::io;
use std::io::Read;
use std::path::Path;

/// As specified in "PEP 513 -- A Platform Tag for Portable Linux Built
/// Distributions"
const MANYLINUX1: &[&str] = &[
    "libpanelw.so.5",
    "libncursesw.so.5",
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

/// As specified in "PEP 571 -- The manylinux2010 Platform Tag"
///
/// Currently unused since the python ecosystem is still on manylinux 1
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

/// Error raised duing auditing an elf file for manylinux compatibility
#[derive(Fail, Debug)]
#[fail(display = "Ensuring manylinux compliancy failed")]
pub enum AuditWheelError {
    /// The wheel couldn't be read
    #[fail(display = "Failed to read the wheel")]
    IOError(#[cause] io::Error),
    /// Reexports elfkit parsing erorrs
    #[fail(display = "Goblin failed to parse the elf file")]
    GoblinError(#[cause] goblin::error::Error),
    /// The elf file isn't manylinux compatible. Contains the list of offending
    /// libraries.
    #[fail(
        display = "Your library is not manylinux compliant because it links the following forbidden libraries: {:?}",
        _0
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
        Manylinux::Manylinux1 => reference = MANYLINUX1,
        Manylinux::Manylinux2010 => reference = MANYLINUX2010,
        _ => return Ok(()),
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
        // I'm not 100% what exactely this line does, but auditwheel also seems to skip
        // everything with ld-linux in its name
        if dep == "ld-linux-x86-64.so.2" || dep == "ld-linux.so.2" {
            continue;
        }
        if !reference.contains(&dep.as_str()) {
            offenders.push(dep);
        }
    }

    if offenders.is_empty() {
        Ok(())
    } else {
        Err(AuditWheelError::ManylinuxValidationError(offenders))
    }
}
