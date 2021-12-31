use crate::common::{check_installed, create_virtualenv, maybe_mock_cargo};
use anyhow::Result;
use maturin::develop;
use std::path::Path;
use std::process::Command;
use std::str;

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// "maturin develop" to install it and checks it is working
pub fn test_develop(
    package: impl AsRef<Path>,
    bindings: Option<String>,
    unique_name: &str,
) -> Result<()> {
    maybe_mock_cargo();

    let (venv_dir, python) = create_virtualenv(&package, "develop")?;

    // Ensure the test doesn't wrongly pass
    check_installed(package.as_ref(), &python).unwrap_err();

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
        vec![
            "--quiet".to_string(),
            "--target-dir".to_string(),
            format!("test-crates/targets/{}", unique_name),
        ],
        vec![],
        &venv_dir,
        false,
        cfg!(feature = "faster-tests"),
        vec![],
    )?;

    check_installed(package.as_ref(), &python)?;
    Ok(())
}
