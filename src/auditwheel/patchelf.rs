use anyhow::{bail, Context, Result};
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Replace a declared dependency on a dynamic library with another one (`DT_NEEDED`)
pub fn replace_needed<O: AsRef<OsStr>, N: AsRef<OsStr>>(
    file: impl AsRef<Path>,
    old_new_pairs: &[(O, N)],
) -> Result<()> {
    let mut cmd = Command::new("patchelf");
    for (old, new) in old_new_pairs {
        cmd.arg("--replace-needed").arg(old).arg(new);
    }
    cmd.arg(file.as_ref());
    let output = cmd
        .output()
        .context("Failed to execute 'patchelf', did you install it? Hint: Try `pip install maturin[patchelf]` (or just `pip install patchelf`)")?;
    if !output.status.success() {
        bail!(
            "patchelf --replace-needed failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Change `SONAME` of a dynamic library
pub fn set_soname<S: AsRef<OsStr>>(file: impl AsRef<Path>, soname: &S) -> Result<()> {
    let mut cmd = Command::new("patchelf");
    cmd.arg("--set-soname").arg(soname).arg(file.as_ref());
    let output = cmd
        .output()
        .context("Failed to execute 'patchelf', did you install it?")?;
    if !output.status.success() {
        bail!(
            "patchelf --set-soname failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Remove a `RPATH` from executables and libraries
pub fn remove_rpath(file: impl AsRef<Path>) -> Result<()> {
    let mut cmd = Command::new("patchelf");
    cmd.arg("--remove-rpath").arg(file.as_ref());
    let output = cmd
        .output()
        .context("Failed to execute 'patchelf', did you install it?")?;
    if !output.status.success() {
        bail!(
            "patchelf --remove-rpath failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Change the `RPATH` of executables and libraries
pub fn set_rpath<S: AsRef<OsStr>>(file: impl AsRef<Path>, rpath: &S) -> Result<()> {
    remove_rpath(&file)?;
    let mut cmd = Command::new("patchelf");
    cmd.arg("--force-rpath")
        .arg("--set-rpath")
        .arg(rpath)
        .arg(file.as_ref());
    let output = cmd
        .output()
        .context("Failed to execute 'patchelf', did you install it?")?;
    if !output.status.success() {
        bail!(
            "patchelf --set-rpath failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Get the `RPATH` of executables and libraries
pub fn get_rpath(file: impl AsRef<Path>) -> Result<Vec<String>> {
    let file = file.as_ref();
    let contents = fs_err::read(&file)?;
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
