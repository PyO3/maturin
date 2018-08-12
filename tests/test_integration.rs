extern crate pyo3_pack;
extern crate target_info;

use pyo3_pack::BuildContext;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use target_info::Target;

// Y U NO accept windows path prefix, pip?
// Anyways, here's shepmasters stack overflow solution
// https://stackoverflow.com/a/50323079/3549270
#[cfg(not(target_os = "windows"))]
fn adjust_canonicalization<P: AsRef<Path>>(p: P) -> String {
    p.as_ref().display().to_string()
}

#[cfg(target_os = "windows")]
fn adjust_canonicalization<P: AsRef<Path>>(p: P) -> String {
    const VERBATIM_PREFIX: &str = r#"\\?\"#;
    let p = p.as_ref().display().to_string();
    if p.starts_with(VERBATIM_PREFIX) {
        p[VERBATIM_PREFIX.len()..].to_string()
    } else {
        p
    }
}

/// For each installed python, this builds a wheel, creates a virtualenv if it doesn't exist,
/// installs get_fourtytwo and runs main.py
#[test]
fn test_integration() {
    let mut options = BuildContext::default();
    options.manifest_path = PathBuf::from("get_fourtytwo/Cargo.toml");
    options.debug = true;
    let (wheels, _) = options.build_wheels().unwrap();
    for (filename, version) in wheels {
        let version = version.unwrap();
        let venv_dir =
            PathBuf::from("get_fourtytwo").join(format!("venv{}.{}", version.major, version.minor));

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
        let pip = if Target::os() == "windows" {
            venv_dir.join("Scripts").join("pip.exe")
        } else {
            venv_dir.join("bin").join("pip")
        };

        let output = Command::new(&pip)
            .args(&["install", "--force-reinstall"])
            .arg(&adjust_canonicalization(filename))
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .unwrap();
        if !output.status.success() {
            panic!();
        }
        let python = if Target::os() == "windows" {
            venv_dir.join("Scripts").join("python.exe")
        } else {
            venv_dir.join("bin").join("python")
        };

        let output = Command::new(&python)
            .arg(Path::new("get_fourtytwo").join("main.py"))
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .unwrap();
        if !output.status.success() {
            panic!();
        }
    }
}
