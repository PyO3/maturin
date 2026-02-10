use crate::common::{
    TestInstallBackend, check_installed, create_conda_env, create_virtualenv, maybe_mock_cargo,
};
use anyhow::Result;
use maturin::{CargoOptions, DevelopOptions, develop};
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
    test_backend: TestInstallBackend,
) -> Result<()> {
    maybe_mock_cargo();

    let package = package.as_ref();
    let (venv_dir, python) = if conda {
        create_conda_env(&format!("maturin-{unique_name}"), 3, 10)?
    } else {
        create_virtualenv(unique_name, None)?
    };

    // Ensure the test doesn't wrongly pass
    check_installed(package, &python).unwrap_err();

    let uv = matches!(test_backend, TestInstallBackend::Uv);
    let mut pip_packages = Vec::new();
    if unique_name.contains("cffi") {
        pip_packages.push("cffi");
    }
    if cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    )) && uv
    {
        pip_packages.push("uv");
    }
    if !pip_packages.is_empty() {
        let mut cmd = Command::new(&python);
        cmd.args(["-m", "pip", "install", "--disable-pip-version-check"])
            .args(pip_packages);
        let output = cmd.output()?;
        if !output.status.success() {
            panic!(
                "Failed to install cffi: {}\n---stdout:\n{}---stderr:\n{}",
                output.status,
                str::from_utf8(&output.stdout)?,
                str::from_utf8(&output.stderr)?
            );
        }
    }

    let manifest_file = package.join("Cargo.toml");
    let develop_options = DevelopOptions {
        bindings,
        release: false,
        strip: false,
        extras: Vec::new(),
        group: Vec::new(),
        skip_install: false,
        pip_path: None,
        cargo_options: CargoOptions {
            manifest_path: Some(manifest_file),
            quiet: true,
            target_dir: Some(PathBuf::from(format!("test-crates/targets/{unique_name}"))),
            ..Default::default()
        },
        uv,
        compression: Default::default(),
    };
    develop(develop_options, &venv_dir)?;

    check_installed(package, &python)?;
    Ok(())
}
