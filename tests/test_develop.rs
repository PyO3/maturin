extern crate pyo3_pack;
extern crate target_info;

use pyo3_pack::develop;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use target_info::Target;

const RUN_GET_FOURTYTWO: &str = r"
import get_fourtytwo

if get_fourtytwo.fourtytwo != 42:
    raise Exception()
";

fn check_get_fourtytwo(python: &PathBuf) -> Result<(), ()> {
    let output = Command::new(&python)
        .args(&["-c", RUN_GET_FOURTYTWO])
        .output()
        .unwrap();
    if !output.status.success() {
        Err(())
    } else {
        Ok(())
    }
}

/// Creates a virtualenv and activates it, checks that get-fourtytwo isn't installed, uses
/// "pyo3-pack develop" on get-fourtytwo and checks it is working
#[test]
fn test_integration() {
    let venv_dir = PathBuf::from("get-fourtytwo").join("venv_develop");

    if venv_dir.is_dir() {
        fs::remove_dir_all(&venv_dir).unwrap();
    }
    let output = Command::new("virtualenv").arg(&venv_dir).output().unwrap();
    if !output.status.success() {
        panic!();
    }

    let python = if Target::os() == "windows" {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    };

    // Ensure the test doesn't wrongly pass
    check_get_fourtytwo(&python).unwrap_err();

    // "activate" the virtualenv
    env::set_var("VIRTUAL_ENV", venv_dir);
    env::set_var(
        "PATH",
        format!(
            "{}:{}",
            python.canonicalize().unwrap().parent().unwrap().display(),
            env::var("PATH").unwrap()
        ),
    );

    let manifest_file = PathBuf::from("get-fourtytwo").join("Cargo.toml");
    develop("pyo3".to_string(), manifest_file, vec![], vec![]).unwrap();

    check_get_fourtytwo(&python).unwrap();
}
