use anyhow::{bail, Context, Result};
use arwen::elf::ElfContainer;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A struct for efficiently patching ELF files with multiple operations
/// while avoiding re-parsing the ELF file multiple times.
pub struct ElfPatcher {
    file_path: PathBuf,
    file_data: Vec<u8>,
}

impl ElfPatcher {
    /// Create a new ElfPatcher by reading and parsing the ELF file
    pub fn new(file: impl AsRef<Path>) -> Result<Self> {
        let file_path = file.as_ref().to_path_buf();
        let file_data = fs_err::read(&file_path).context("Failed to read ELF file")?;

        // Validate that it's a valid ELF file
        ElfContainer::parse(&file_data)
            .map_err(|e| anyhow::anyhow!("Failed to parse ELF file: {}", e))?;

        Ok(Self {
            file_path,
            file_data,
        })
    }

    /// Replace declared dependencies on dynamic libraries with new ones (DT_NEEDED)
    pub fn replace_needed<O: AsRef<OsStr>, N: AsRef<OsStr>>(
        &mut self,
        old_new_pairs: &[(O, N)],
    ) -> Result<&mut Self> {
        let mut container = ElfContainer::parse(&self.file_data)
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

        self.file_data = container.data;
        Ok(self)
    }

    /// Change the SONAME of a dynamic library
    pub fn set_soname<S: AsRef<OsStr>>(&mut self, soname: &S) -> Result<&mut Self> {
        let mut container = ElfContainer::parse(&self.file_data)
            .map_err(|e| anyhow::anyhow!("Failed to parse ELF file: {}", e))?;

        let soname_str = soname.as_ref().to_string_lossy();
        container
            .set_soname(&soname_str)
            .map_err(|e| anyhow::anyhow!("Failed to set soname: {}", e))?;

        self.file_data = container.data;
        Ok(self)
    }

    /// Remove the RPATH from the ELF file
    pub fn remove_rpath(&mut self) -> Result<&mut Self> {
        let mut container = ElfContainer::parse(&self.file_data)
            .map_err(|e| anyhow::anyhow!("Failed to parse ELF file: {}", e))?;

        container
            .remove_runpath()
            .map_err(|e| anyhow::anyhow!("Failed to remove rpath: {}", e))?;

        self.file_data = container.data;
        Ok(self)
    }

    /// Set the RPATH of the ELF file
    pub fn set_rpath<S: AsRef<OsStr>>(&mut self, rpath: &S) -> Result<&mut Self> {
        let mut container = ElfContainer::parse(&self.file_data)
            .map_err(|e| anyhow::anyhow!("Failed to parse ELF file: {}", e))?;

        let rpath_str = rpath.as_ref().to_string_lossy();
        container
            .set_runpath(&rpath_str)
            .map_err(|e| anyhow::anyhow!("Failed to set rpath: {}", e))?;

        self.file_data = container.data;
        Ok(self)
    }

    /// Modify the RPATH using a closure that transforms the current rpath list
    pub fn modify_rpath<F>(&mut self, modifier: F) -> Result<&mut Self>
    where
        F: FnOnce(Vec<String>) -> Vec<String>,
    {
        // Get the old rpath using goblin (we could parse once and use both goblin and arwen, but this is simpler)
        let old_rpaths = get_rpath(&self.file_path)?;

        // Apply the modifier function
        let new_rpaths = modifier(old_rpaths);
        let new_rpath = new_rpaths.join(":");

        let mut container = ElfContainer::parse(&self.file_data)
            .map_err(|e| anyhow::anyhow!("Failed to parse ELF file: {}", e))?;

        // First remove existing rpath
        container
            .remove_runpath()
            .map_err(|e| anyhow::anyhow!("Failed to remove existing rpath: {}", e))?;

        // Then set the new rpath
        container
            .set_runpath(&new_rpath)
            .map_err(|e| anyhow::anyhow!("Failed to set rpath: {}", e))?;

        self.file_data = container.data;
        Ok(self)
    }

    /// Save the modified ELF file back to disk
    pub fn save(&self) -> Result<()> {
        fs_err::write(&self.file_path, &self.file_data).context("Failed to write modified ELF file")
    }
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
