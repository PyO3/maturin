use anyhow::{Context, Result};
use fs_err as fs;
use goblin::elf::Elf;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Find musl libc path from executable's ELF header
pub fn find_musl_libc() -> Result<Option<PathBuf>> {
    let buffer = fs::read("/bin/ls")?;
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
    let output = Command::new(&ld_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    let stderr = std::str::from_utf8(&output.stderr)?;
    let expr = Regex::new(r"Version (\d+)\.(\d+)")?;
    if let Some(capture) = expr.captures(stderr) {
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
