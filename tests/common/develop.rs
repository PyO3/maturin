use crate::common::{check_installed, create_conda_env, create_virtualenv, maybe_mock_cargo};
use anyhow::Result;
use maturin::{develop, CargoOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str;

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// "maturin develop" to install it and checks it is working
pub fn test_develop(
    package: impl AsRef<Path>,
    bindings: Option<String>,
    unique_name: &str,
    conda: bool,
) -> Result<()> {
    maybe_mock_cargo();

    let package = package.as_ref();
    let (venv_dir, python) = if conda {
        create_conda_env(&format!("maturin-{}", unique_name), 3, 10)?
    } else {
        create_virtualenv(unique_name, None)?
    };

    // Ensure the test doesn't wrongly pass
    check_installed(package, &python).unwrap_err();

    let output = Command::new(&python)
        .args([
            "-m",
            "pip",
            "install",
            "--disable-pip-version-check",
            "cffi",
        ])
        .output()?;
    if !output.status.success() {
        panic!(
            "Failed to install cffi: {}\n---stdout:\n{}---stderr:\n{}",
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    let manifest_file = package.join("Cargo.toml");
    develop(
        bindings,
        CargoOptions {
            manifest_path: Some(manifest_file),
            quiet: true,
            target_dir: Some(PathBuf::from(format!(
                "test-crates/targets/{}",
                unique_name
            ))),
            ..Default::default()
        },
        &venv_dir,
        false,
        cfg!(feature = "faster-tests"),
        vec![],
    )?;

    check_installed(package, &python)?;
    Ok(())
}
