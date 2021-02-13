use crate::common::{adjust_canonicalization, check_installed, maybe_mock_cargo};
use anyhow::Result;
use fs_err as fs;
use maturin::{develop, Target};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::str;

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// "maturin develop" to install it and checks it is working
pub fn test_develop(package: impl AsRef<Path>, bindings: Option<String>) -> Result<()> {
    maybe_mock_cargo();

    let test_name = package
        .as_ref()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let venv_dir = PathBuf::from("test-crates")
        .canonicalize()?
        .join("venvs")
        .join(format!("{}-develop", test_name));
    let target = Target::from_target_triple(None)?;

    if venv_dir.is_dir() {
        fs::remove_dir_all(&venv_dir)?;
    }

    let output = Command::new("virtualenv")
        .arg(adjust_canonicalization(&venv_dir))
        .stderr(Stdio::inherit())
        .output()
        .expect("Failed to create a virtualenv");
    if !output.status.success() {
        panic!(
            "Failed to run virtualenv: {}\n---stdout:\n{}---stderr:\n{}",
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    let python = target.get_venv_python(&venv_dir);

    // Ensure the test doesn't wrongly pass
    check_installed(&package.as_ref(), &python).unwrap_err();

    let output = Command::new(&python)
        .args(&["-m", "pip", "install", "-U", "pip", "cffi"])
        .output()?;
    if !output.status.success() {
        panic!(
            "Failed to install cffi: {}\n---stdout:\n{}---stderr:\n{}",
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    let manifest_file = package.as_ref().join("Cargo.toml");
    develop(
        bindings,
        &manifest_file,
        vec!["--quiet".to_string()],
        vec![],
        &venv_dir,
        false,
        cfg!(feature = "faster-tests"),
    )?;

    check_installed(&package.as_ref(), &python)?;
    Ok(())
}
