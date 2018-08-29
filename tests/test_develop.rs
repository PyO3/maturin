extern crate pyo3_pack;

use common::check_installed;
use pyo3_pack::{develop, Target};
use std::fs;
use std::path::Path;
use std::process::Command;

mod common;

#[test]
fn test_develop_get_fourtytwo() {
    test_develop(Path::new("get-fourtytwo"));
}

#[test]
fn test_develop_points() {
    test_develop(Path::new("points"));
}

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// "pyo3-pack develop" to install it and checks it is working
fn test_develop(package: &Path) {
    let venv_dir = package.join("venv_develop");
    let target = Target::current();

    if venv_dir.is_dir() {
        fs::remove_dir_all(&venv_dir).unwrap();
    }
    let output = Command::new("virtualenv").arg(&venv_dir).output().expect(
        "You need to have virtualenv installed to run the tests (`pip install virtualenv`)",
    );
    if !output.status.success() {
        panic!();
    }

    let python = target.get_venv_python(&venv_dir);

    // Ensure the test doesn't wrongly pass
    check_installed(&package, &python).unwrap_err();

    let output = Command::new(&python)
        .args(&["-m", "pip", "install", "cffi"])
        .output()
        .unwrap();
    if !output.status.success() {
        panic!();
    }

    let manifest_file = package.join("Cargo.toml");
    develop(&None, &manifest_file, vec![], vec![], &venv_dir).unwrap();

    check_installed(&package, &python).unwrap();
}
