use crate::common::{check_installed, handle_result};
use failure::Error;
use failure::ResultExt;
use pyo3_pack::{develop, Target};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
mod common;

#[test]
fn test_develop_get_fourtytwo() {
    handle_result(test_develop(Path::new("get-fourtytwo"), None));
}

#[test]
fn test_develop_points() {
    handle_result(test_develop(Path::new("points"), Some("cffi".to_string())));
}

#[test]
fn test_develop_hello_world() {
    handle_result(test_develop(
        Path::new("hello-world"),
        Some("bin".to_string()),
    ));
}

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// "pyo3-pack develop" to install it and checks it is working
fn test_develop(package: &Path, bindings: Option<String>) -> Result<(), Error> {
    let venv_dir = package
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
        .expect(
            "You need to have virtualenv installed to run the tests (`pip install virtualenv`)",
        );
    if !output.status.success() {
        panic!(output.status);
    }

    let python = target.get_venv_python(&venv_dir);

    // Ensure the test doesn't wrongly pass
    check_installed(&package, &python).unwrap_err();

    let output = Command::new(&python)
        .args(&["-m", "pip", "install", "cffi"])
        .output()?;
    if !output.status.success() {
        panic!("Failed to install cffi: {}", output.status);
    }

    let manifest_file = package.join("Cargo.toml");
    develop(
        bindings,
        &manifest_file,
        vec!["--quiet".to_string()],
        vec![],
        &venv_dir,
        false,
        false,
    )?;

    check_installed(&package, &python)?;
    Ok(())
}
