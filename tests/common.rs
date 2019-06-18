use failure::{bail, Error};
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str;

/// Check that the package is either not installed or works correctly
pub fn check_installed(package: &Path, python: &PathBuf) -> Result<(), Error> {
    let check_installed = Path::new(package)
        .join("check_installed")
        .join("check_installed.py");
    let output = Command::new(&python)
        .arg(check_installed)
        .env("PATH", python.parent().unwrap())
        .output()
        .unwrap();
    if !output.status.success() {
        bail!(
            "Check install fail: {} \n--- Stdout:\n{}\n--- Stderr:\n{}",
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    let message = str::from_utf8(&output.stdout).unwrap().trim();

    if message != "SUCCESS" {
        panic!("{}", message);
    }

    Ok(())
}

pub fn handle_result<T>(result: Result<T, Error>) {
    if let Err(e) = result {
        for cause in e.as_fail().iter_chain().collect::<Vec<_>>().iter().rev() {
            eprintln!("{}", cause);
        }
        panic!("{}", e);
    }
}
