use super::audit::{find_versioned_libraries, AuditWheelError};
use anyhow::Result;
use fs_err as fs;
use goblin::elf::Elf;
use std::io::Read;
use std::path::Path;

pub fn repair(artifact: impl AsRef<Path>) -> Result<(), AuditWheelError> {
    let mut file = fs::File::open(artifact.as_ref()).map_err(AuditWheelError::IoError)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(AuditWheelError::IoError)?;
    let elf = Elf::parse(&buffer).map_err(AuditWheelError::GoblinError)?;
    let ext_libs = find_versioned_libraries(&elf);
    for lib in ext_libs {
        println!("{}", lib.name);
    }
    Ok(())
}
