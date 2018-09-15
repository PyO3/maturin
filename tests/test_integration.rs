extern crate pyo3_pack;

use common::check_installed;
use pyo3_pack::{BuildOptions, Target};
use std::path::Path;
use std::process::{Command, Stdio};

mod common;

// Y U NO accept windows path prefix, pip?
// Anyways, here's shepmasters stack overflow solution
// https://stackoverflow.com/a/50323079/3549270
#[cfg(not(target_os = "windows"))]
fn adjust_canonicalization(p: impl AsRef<Path>) -> String {
    p.as_ref().display().to_string()
}

#[cfg(target_os = "windows")]
fn adjust_canonicalization(p: impl AsRef<Path>) -> String {
    const VERBATIM_PREFIX: &str = r#"\\?\"#;
    let p = p.as_ref().display().to_string();
    if p.starts_with(VERBATIM_PREFIX) {
        p[VERBATIM_PREFIX.len()..].to_string()
    } else {
        p
    }
}

#[test]
fn test_integration_get_fourtytwo() {
    test_integration(Path::new("get-fourtytwo"), None);
}

#[test]
fn test_integration_points() {
    test_integration(Path::new("points"), Some("cffi".to_string()));
}

#[test]
fn test_integration_hello_world() {
    test_integration(Path::new("hello-world"), Some("bin".to_string()));
}

/// For each installed python version, this builds a wheel, creates a virtualenv if it
/// doesn't exist, installs the package and runs check_installed.py
fn test_integration(package: &Path, bindings: Option<String>) {
    let target = Target::current();

    let mut options = BuildOptions::default();
    options.manifest_path = package.join("Cargo.toml");
    options.bindings = bindings;
    options.debug = true;

    let wheels = options
        .into_build_context()
        .unwrap()
        .build_wheels()
        .unwrap();

    for (filename, supported_version, python_interpreter) in wheels {
        let venv_dir = if supported_version == "py2.py3" {
            package.join("venv_cffi")
        } else {
            package.join(format!("venv{}.{}", supported_version.chars().nth(2usize).unwrap(), supported_version.chars().nth(3usize).unwrap()))
        };

        if !venv_dir.is_dir() {
            let venv_py_version = if let Some(ref python_interpreter) = python_interpreter {
                python_interpreter.executable.clone()
            } else {
                target.get_python()
            };

            let output = Command::new("virtualenv")
                .args(&[
                    "-p",
                    &venv_py_version.display().to_string(),
                    &venv_dir.display().to_string(),
                ]).stderr(Stdio::inherit())
                .output()
                .unwrap();
            if !output.status.success() {
                panic!();
            }
        }

        let python = target.get_venv_python(&venv_dir);

        let output = Command::new(&python)
            .args(&[
                "-m",
                "pip",
                "install",
                "--force-reinstall",
                &adjust_canonicalization(filename),
            ]).stderr(Stdio::inherit())
            .output()
            .unwrap();
        if !output.status.success() {
            panic!();
        }

        let output = Command::new(&python)
            .args(&["-m", "pip", "install", "cffi"])
            .output()
            .unwrap();
        if !output.status.success() {
            panic!();
        }

        check_installed(&package, &python).unwrap();
    }
}
