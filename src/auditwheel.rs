use elfkit;
use std::fs::File;
use std::io;
use std::path::Path;

// As specified in "PEP 513 -- A Platform Tag for Portable Linux Built
// Distributions"
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
#[allow(unused)]
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
    #[fail(display = "Elfkit failed to parse the elf file: {:?}", _0)]
    ElfkitError(elfkit::Error),
    /// The elf file isn't manylinux compatible. Contains the list of offending
    /// libraries.
    #[fail(
        display = "Your library is not manylinux compliant because it links the following forbidden libraries: {:?}",
        _0
    )]
    ManylinuxValidationError(Vec<String>),
}

/// Similar to the `ldd` command: Returns all libraries that an elf dynamically
/// links to
fn get_deps_from_elf(path: &Path) -> Result<Vec<String>, AuditWheelError> {
    let mut file = File::open(path).map_err(AuditWheelError::IOError)?;
    let mut elf = elfkit::Elf::from_reader(&mut file).map_err(AuditWheelError::ElfkitError)?;
    elf.load_all(&mut file)
        .map_err(AuditWheelError::ElfkitError)?;

    let mut deps = Vec::new();
    for section in elf.sections {
        if let elfkit::SectionContent::Dynamic(dynamic) = section.content {
            for dyn in dynamic {
                if dyn.dhtype == elfkit::types::DynamicType::NEEDED {
                    if let elfkit::DynamicContent::String(ref name) = dyn.content {
                        deps.push(String::from_utf8_lossy(&name.0).into_owned());
                    }
                }
            }
        }
    }

    Ok(deps)
}

/// An (incomplete) reimplementation of auditwheel, which checks elf files for
/// manylinux compliance
///
/// Only checks for the libraries marked as NEEDED.
pub fn auditwheel_rs(path: &Path) -> Result<(), AuditWheelError> {
    let deps = get_deps_from_elf(&path)?;

    let mut offenders = Vec::new();
    for dep in deps {
        // I'm not 100% what exactely this line does, but auditwheel also seems to skip
        // everything with ld-linux in its name
        if dep == "ld-linux-x86-64.so.2" {
            continue;
        }
        if !MANYLINUX1.contains(&dep.as_str()) {
            offenders.push(dep);
        }
    }

    if offenders.is_empty() {
        Ok(())
    } else {
        Err(AuditWheelError::ManylinuxValidationError(offenders))
    }
}
