use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str;
use std::process::Stdio;

/// Check that the package is either not installed or works correctly
pub fn check_installed(package: &Path, python: &PathBuf) -> Result<(), ()> {
    let output = Command::new(&python)
        .arg(Path::new(package).join("check_installed.py"))
        .env("PATH", python.parent().unwrap())
        .output()
        .unwrap();
    if !output.status.success() {
        return Err(());
    }

    let message = str::from_utf8(&output.stdout).unwrap().trim();

    if message != "SUCCESS" {
        panic!();
    }

    Ok(())
}

pub fn install_cffi(python: &PathBuf) {
    let output = Command::new(&python)
        .args(&["-m", "pip", "install", "cffi"])
        .stderr(Stdio::inherit())
        .output()
        .unwrap();
    if !output.status.success() {
        panic!(output.status);
    }
}
