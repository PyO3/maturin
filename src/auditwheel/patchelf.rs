use anyhow::{Result, bail};
use arwen::elf::ElfContainer;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

/// Replace a declared dependency on a dynamic library with another one (`DT_NEEDED`)
pub fn replace_needed<O: AsRef<OsStr>, N: AsRef<OsStr>>(
    file: impl AsRef<Path>,
    old_new_pairs: &[(O, N)],
) -> Result<()> {
    let bytes_of_file = fs_err::read(file.as_ref())?;
    let mut elf = ElfContainer::parse(&bytes_of_file)?;

    let mut dt_needed = HashMap::new();
    for (key, value) in old_new_pairs {
        dt_needed.insert(
            key.as_ref().as_encoded_bytes(),
            value.as_ref().as_encoded_bytes(),
        );
    }

    elf.replace_needed(&dt_needed)?;

    elf.write_to_path(file.as_ref())?;

    Ok(())
}

/// Change `SONAME` of a dynamic library
pub fn set_soname<S: AsRef<OsStr>>(file: impl AsRef<Path>, soname: &S) -> Result<()> {
    let bytes_of_file = fs_err::read(file.as_ref())?;
    let mut elf = ElfContainer::parse(&bytes_of_file)?;

    elf.set_soname(soname.as_ref().as_encoded_bytes())?;

    elf.write_to_path(file.as_ref())?;

    Ok(())
}

/// Remove a `RPATH` from executables and libraries
pub fn remove_rpath(file: impl AsRef<Path>) -> Result<()> {
    let bytes_of_file = fs_err::read(file.as_ref())?;
    let mut elf = ElfContainer::parse(&bytes_of_file)?;

    elf.remove_runpath()?;

    elf.write_to_path(file.as_ref())?;

    Ok(())
}

/// Change the `RPATH` of executables and libraries
pub fn set_rpath<S: AsRef<OsStr>>(file: impl AsRef<Path>, rpath: &S) -> Result<()> {
    remove_rpath(&file)?;
    let bytes_of_file = fs_err::read(file.as_ref())?;
    let mut elf = ElfContainer::parse(&bytes_of_file)?;

    elf.add_runpath(rpath.as_ref().as_encoded_bytes())?;
    elf.force_rpath()?;

    elf.write_to_path(file.as_ref())?;

    Ok(())
}

/// Get the `RPATH` of executables and libraries
pub fn get_rpath(file: impl AsRef<Path>) -> Result<Vec<String>> {
    let file = file.as_ref();
    let contents = fs_err::read(file)?;
    match goblin::Object::parse(&contents) {
        Ok(goblin::Object::Elf(elf)) => {
            let rpaths = if !elf.runpaths.is_empty() {
                elf.runpaths
            } else {
                elf.rpaths
            };
            Ok(rpaths.iter().map(|r| r.to_string()).collect())
        }
        Ok(_) => bail!("'{}' is not an ELF file", file.display()),
        Err(e) => bail!("Failed to parse ELF file at '{}': {}", file.display(), e),
    }
}
