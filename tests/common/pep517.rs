use crate::common::{check_installed, create_conda_env, create_virtualenv, maybe_mock_cargo};
use anyhow::Result;
use std::path::Path;
use std::process::{Command, Output};
use std::str;
use tempfile::tempdir;

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// pip install to install it and checks it is working
pub fn test_pep517(
    package: impl AsRef<Path>,
    unique_name: &str,
    conda: bool,
    editable: bool,
) -> Result<Output> {
    maybe_mock_cargo();

    let package = package.as_ref();
    let (_venv_dir, python) = if conda {
        create_conda_env(&format!("maturin-{unique_name}"), 3, 10)?
    } else {
        create_virtualenv(unique_name, None)?
    };

    // Ensure the test doesn't wrongly pass
    check_installed(package, &python).unwrap_err();

    let build_dir = tempdir().unwrap();

    // install maturin in the venv
    let output = Command::new(&python)
        .args([
            "-m",
            "pip",
            "install",
            env!("CARGO_MANIFEST_DIR"),
            // ensure that each `maturin` build for the bdist is within an isolated dir
            // this ensures tests do not race with each other
            // the Rust `target/` dir is still reused and will have Cargo's locking
            "--config-settings=--global-option=build",
            format!(
                "--config-settings=--global-option=--build-base={}",
                build_dir.path().display()
            )
            .as_str(),
        ])
        .env("SETUPTOOLS_RUST_CARGO_PROFILE", "dev")
        .output()?;

    // drop(build_dir);

    if !output.status.success() {
        panic!(
            "Failed to install maturin: {}\n---stdout:\n{}---stderr:\n{}",
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    let mut cmd = Command::new(&python);
    cmd.args(["-m", "pip", "install", "-vvv", "--no-build-isolation"]);

    if editable {
        cmd.arg("--editable");
    }

    cmd.arg(package.to_str().expect("package is utf8 path"));

    cmd.env(
        "CARGO_TARGET_DIR",
        format!(
            "{}/test-crates/targets/{unique_name}",
            env!("CARGO_MANIFEST_DIR")
        ),
    );

    let output = cmd.output()?;
    if !output.status.success() {
        panic!(
            "Failed to install {}: {}\n---stdout:\n{}---stderr:\n{}",
            package.display(),
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    check_installed(package, &python)?;
    Ok(output)
}
