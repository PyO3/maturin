use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

static MISSING_PATCHELF_ERROR: &str = "Failed to execute 'patchelf', did you install it? Hint: Try `pip install maturin[patchelf]` (or just `pip install patchelf`)";

/// Run a patchelf command with the given arguments.
///
/// Returns `Ok(stdout)` on success, or an error with the stderr message.
fn run_patchelf(args: &[&OsStr]) -> Result<Vec<u8>> {
    let output = Command::new("patchelf")
        .args(args)
        .output()
        .context(MISSING_PATCHELF_ERROR)?;
    if !output.status.success() {
        bail!(
            "patchelf failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}

/// Verify patchelf version
pub fn verify_patchelf() -> Result<()> {
    let stdout = run_patchelf(&[OsStr::new("--version")])?;
    let version = String::from_utf8(stdout)
        .context("Failed to parse patchelf version")?
        .trim()
        .to_string();
    let version = version.strip_prefix("patchelf").unwrap_or(&version).trim();
    let semver = version.parse::<semver::Version>().context(
        "Failed to parse patchelf version, auditwheel repair requires patchelf >= 0.14.0.",
    )?;
    if semver < semver::Version::new(0, 14, 0) {
        bail!(
            "patchelf {} found. auditwheel repair requires patchelf >= 0.14.0.",
            version
        );
    }
    Ok(())
}

/// Replace a declared dependency on a dynamic library with another one (`DT_NEEDED`)
pub fn replace_needed<O: AsRef<OsStr>, N: AsRef<OsStr>>(
    file: impl AsRef<Path>,
    old_new_pairs: &[(O, N)],
) -> Result<()> {
    let mut args: Vec<&OsStr> = Vec::new();
    for (old, new) in old_new_pairs {
        args.push(OsStr::new("--replace-needed"));
        args.push(old.as_ref());
        args.push(new.as_ref());
    }
    args.push(file.as_ref().as_os_str());
    run_patchelf(&args)?;
    Ok(())
}

/// Change `SONAME` of a dynamic library
pub fn set_soname<S: AsRef<OsStr>>(file: impl AsRef<Path>, soname: &S) -> Result<()> {
    run_patchelf(&[
        OsStr::new("--set-soname"),
        soname.as_ref(),
        file.as_ref().as_os_str(),
    ])?;
    Ok(())
}

/// Remove a `RPATH` from executables and libraries
pub fn remove_rpath(file: impl AsRef<Path>) -> Result<()> {
    run_patchelf(&[OsStr::new("--remove-rpath"), file.as_ref().as_os_str()])?;
    Ok(())
}

/// Change the `RPATH` of executables and libraries
pub fn set_rpath<S: AsRef<OsStr>>(file: impl AsRef<Path>, rpath: &S) -> Result<()> {
    remove_rpath(&file)?;
    run_patchelf(&[
        OsStr::new("--force-rpath"),
        OsStr::new("--set-rpath"),
        rpath.as_ref(),
        file.as_ref().as_os_str(),
    ])?;
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
