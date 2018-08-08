extern crate pyo3_pack;

use pyo3_pack::BuildContext;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// For each installed python, this builds a wheel, creates a virtualenv if it doesn't exist,
/// installs get_fourtytwo and runs main.py
#[test]
fn test_integration() {
    let mut options = BuildContext::default();
    options.manifest_path = PathBuf::from("get_fourtytwo/Cargo.toml");
    let (wheels, _) = options.build_wheels().unwrap();
    for (filename, version) in wheels {
        let version = version.unwrap();
        let venv_dir = PathBuf::from(format!(
            "get_fourtytwo/venv{}.{}",
            version.major, version.minor
        ));

        if !venv_dir.is_dir() {
            let output = Command::new("virtualenv")
                .args(&[
                    "-p",
                    &version.executable.display().to_string(),
                    &venv_dir.display().to_string(),
                ]).stderr(Stdio::inherit())
                .output()
                .unwrap();
            if !output.status.success() {
                panic!();
            }
        }

        let output = Command::new(&venv_dir.join("bin").join("pip"))
            .args(&[
                "install",
                "--force-reinstall",
                &filename.display().to_string(),
            ]).stderr(Stdio::inherit())
            .output()
            .unwrap();
        if !output.status.success() {
            panic!();
        }
        let output = Command::new(&venv_dir.join("bin").join("python"))
            .arg("get_fourtytwo/main.py")
            .stderr(Stdio::inherit())
            .output()
            .unwrap();
        if !output.status.success() {
            panic!();
        }
    }
}
