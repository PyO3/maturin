use anyhow::{bail, Context, Result};
use arwen::elf::ElfContainer;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

/// Verify arwen is available (no version requirement as it's a library)
pub fn verify_patchelf() -> Result<()> {
    // Since we're using arwen as a library, there's no external tool to verify
    // This function now serves as a compatibility shim and always succeeds
    Ok(())
}

/// Replace a declared dependency on a dynamic library with another one (`DT_NEEDED`)
pub fn replace_needed<O: AsRef<OsStr>, N: AsRef<OsStr>>(
    file: impl AsRef<Path>,
    old_new_pairs: &[(O, N)],
) -> Result<()> {
    let file_path = file.as_ref();
    let file_data = fs_err::read(file_path).context("Failed to read ELF file")?;

    let mut container = ElfContainer::parse(&file_data)
        .map_err(|e| anyhow::anyhow!("Failed to parse ELF file: {}", e))?;

    // Convert old_new_pairs to HashMap<String, String> as expected by arwen
    let mut replacements = HashMap::new();
    for (old, new) in old_new_pairs {
        let old_str = old.as_ref().to_string_lossy().to_string();
        let new_str = new.as_ref().to_string_lossy().to_string();
        replacements.insert(old_str, new_str);
    }

    container
        .replace_needed(&replacements)
        .map_err(|e| anyhow::anyhow!("Failed to replace needed libraries: {}", e))?;

    // Write the modified file back
    fs_err::write(file_path, &container.data).context("Failed to write modified ELF file")?;

    Ok(())
}

/// Change `SONAME` of a dynamic library
pub fn set_soname<S: AsRef<OsStr>>(file: impl AsRef<Path>, soname: &S) -> Result<()> {
    let file_path = file.as_ref();
    let file_data = fs_err::read(file_path).context("Failed to read ELF file")?;

    let mut container = ElfContainer::parse(&file_data)
        .map_err(|e| anyhow::anyhow!("Failed to parse ELF file: {}", e))?;

    let soname_str = soname.as_ref().to_string_lossy();
    container
        .set_soname(&soname_str)
        .map_err(|e| anyhow::anyhow!("Failed to set soname: {}", e))?;

    // Write the modified file back
    fs_err::write(file_path, &container.data).context("Failed to write modified ELF file")?;

    Ok(())
}

/// Remove a `RPATH` from executables and libraries
pub fn remove_rpath(file: impl AsRef<Path>) -> Result<()> {
    let file_path = file.as_ref();
    let file_data = fs_err::read(file_path).context("Failed to read ELF file")?;

    let mut container = ElfContainer::parse(&file_data)
        .map_err(|e| anyhow::anyhow!("Failed to parse ELF file: {}", e))?;

    container
        .remove_runpath()
        .map_err(|e| anyhow::anyhow!("Failed to remove rpath: {}", e))?;

    // Write the modified file back
    fs_err::write(file_path, &container.data).context("Failed to write modified ELF file")?;

    Ok(())
}

/// Change the `RPATH` of executables and libraries
pub fn set_rpath<S: AsRef<OsStr>>(file: impl AsRef<Path>, rpath: &S) -> Result<()> {
    let file_path = file.as_ref();
    let file_data = fs_err::read(file_path).context("Failed to read ELF file")?;

    let mut container = ElfContainer::parse(&file_data)
        .map_err(|e| anyhow::anyhow!("Failed to parse ELF file: {}", e))?;

    // First remove existing rpath
    container
        .remove_runpath()
        .map_err(|e| anyhow::anyhow!("Failed to remove existing rpath: {}", e))?;

    // Then set the new rpath
    let rpath_str = rpath.as_ref().to_string_lossy();
    container
        .set_runpath(&rpath_str)
        .map_err(|e| anyhow::anyhow!("Failed to set rpath: {}", e))?;

    // Write the modified file back
    fs_err::write(file_path, &container.data).context("Failed to write modified ELF file")?;

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
