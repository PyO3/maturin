extern crate pyo3_pack;

use common::check_installed;
use pyo3_pack::{develop, Target};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;

mod common;

#[test]
fn test_develop_get_fourtytwo() {
    test_develop(Path::new("get-fourtytwo"), None);
}

#[test]
fn test_develop_points() {
    test_develop(Path::new("points"), Some("cffi".to_string()));
}

#[test]
fn test_develop_hello_world() {
    test_develop(Path::new("hello-world"), Some("bin".to_string()));
}

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// "pyo3-pack develop" to install it and checks it is working
fn test_develop(package: &Path, bindings: Option<String>) {
    let venv_dir = package.join("venv_develop");
    let target = Target::current();

    if venv_dir.is_dir() {
        fs::remove_dir_all(&venv_dir).unwrap();
    }
    let output = Command::new("virtualenv")
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
        .output()
        .unwrap();
    if !output.status.success() {
        panic!(output.status);
    }

    let manifest_file = package.join("Cargo.toml");
    develop(bindings, &manifest_file, vec![], vec![], &venv_dir, false).unwrap();

    check_installed(&package, &python).unwrap();
}
