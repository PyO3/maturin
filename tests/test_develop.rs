use crate::common::{check_installed, handle_result, maybe_mock_cargo};
use failure::Error;
use failure::ResultExt;
use pyo3_pack::{develop, Target};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::str;
mod common;

#[cfg(not(feature = "skip-nightly-tests"))]
#[test]
fn test_develop_pyo3_pure() {
    handle_result(test_develop("test-crates/pyo3-pure", None));
}

#[cfg(not(feature = "skip-nightly-tests"))]
#[test]
fn test_develop_pyo3_mixed() {
    handle_result(test_develop("test-crates/pyo3-mixed", None));
}

#[test]
fn test_develop_cffi_pure() {
    handle_result(test_develop(
        "test-crates/cffi-pure",
        Some("cffi".to_string()),
    ));
}

#[test]
fn test_develop_cffi_mixed() {
    handle_result(test_develop(
        "test-crates/cffi-mixed",
        Some("cffi".to_string()),
    ));
}

#[test]
fn test_develop_hello_world() {
    handle_result(test_develop(
        "test-crates/hello-world",
        Some("bin".to_string()),
    ));
}

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// "pyo3-pack develop" to install it and checks it is working
fn test_develop(package: impl AsRef<Path>, bindings: Option<String>) -> Result<(), Error> {
    maybe_mock_cargo();

    let venv_dir = package
        .as_ref()
        .canonicalize()
        .context("package dir doesn't exist")?
        .join("venv_develop");
    let target = Target::from_target_triple(None)?;

    if venv_dir.is_dir() {
        fs::remove_dir_all(&venv_dir)?;
    }
    let output = Command::new("python3")
        .arg("-m")
        .arg("venv")
        .arg(&venv_dir)
        .stderr(Stdio::inherit())
        .output()
        .expect("Failed to run python to create a virtualenv");
    if !output.status.success() {
        panic!(output.status);
    }

    let python = target.get_venv_python(&venv_dir);

    // Ensure the test doesn't wrongly pass
    check_installed(&package.as_ref(), &python).unwrap_err();

    let output = Command::new(&python)
        .args(&["-m", "pip", "install", "cffi"])
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
