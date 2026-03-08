use anyhow::{Context, Result};
use fs_err as fs;
use goblin::elf::Elf;
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Find musl libc path from executable's ELF header
pub fn find_musl_libc() -> Result<Option<PathBuf>> {
    // Try /bin/ls first; fall back to /usr/bin/ls for distros that don't
    // symlink /bin -> /usr/bin.
    let ls_path = if Path::new("/bin/ls").exists() {
        Path::new("/bin/ls")
    } else {
        Path::new("/usr/bin/ls")
    };
    let buffer = fs::read(ls_path)?;
    let elf = Elf::parse(&buffer)?;
    Ok(elf.interpreter.map(PathBuf::from))
}

/// Read the musl version from libc library's output
///
/// The libc library should output something like this to stderr::
///
/// musl libc (x86_64)
/// Version 1.2.2
/// Dynamic Program Loader
pub fn get_musl_version(ld_path: impl AsRef<Path>) -> Result<Option<(u16, u16)>> {
    let ld_path = ld_path.as_ref();
    let output = Command::new(ld_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    let stderr = std::str::from_utf8(&output.stderr)?;
    static VERSION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"Version (\d+)\.(\d+)").unwrap());
    if let Some(capture) = VERSION_RE.captures(stderr) {
        let context = "Expected a digit";
        let major = capture
            .get(1)
            .unwrap()
            .as_str()
            .parse::<u16>()
            .context(context)?;
        let minor = capture
            .get(2)
            .unwrap()
            .as_str()
            .parse::<u16>()
            .context(context)?;
        return Ok(Some((major, minor)));
    }
    Ok(None)
}
