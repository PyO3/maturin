use anyhow::{bail, Context, Result};
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Replace a declared dependency on a dynamic library with another one (`DT_NEEDED`)
pub fn replace_needed<S: AsRef<OsStr>>(
    file: impl AsRef<Path>,
    old_lib: &str,
    new_lib: &S,
) -> Result<()> {
    let mut cmd = Command::new("patchelf");
    cmd.arg("--replace-needed")
        .arg(old_lib)
        .arg(new_lib)
        .arg(file.as_ref());
    let output = cmd
        .output()
        .context("Failed to execute 'patchelf', did you install it?")?;
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

/// /// Remove a `RPATH` from executables and libraries
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
pub fn get_rpath(file: impl AsRef<Path>) -> Result<String> {
    let mut cmd = Command::new("patchelf");
    cmd.arg("--print-rpath").arg(file.as_ref());
    let output = cmd
        .output()
        .context("Failed to execute 'patchelf', did you install it?")?;
    if !output.status.success() {
        bail!(
            "patchelf --print-rpath failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let rpath = String::from_utf8(output.stdout)?;
    Ok(rpath.trim().to_string())
}
