use crate::common::{
    adjust_canonicalization, check_installed, create_virtualenv, maybe_mock_cargo,
};
use anyhow::{bail, Context, Result};
use maturin::BuildOptions;
use std::path::Path;
use std::process::Command;
use std::str;
use structopt::StructOpt;

/// test PEP 660 editable installs
pub fn test_editable(package: impl AsRef<Path>, bindings: Option<String>) -> Result<()> {
    maybe_mock_cargo();

    let package_string = package.as_ref().join("Cargo.toml").display().to_string();

    let (venv_dir, python) = create_virtualenv(&package, "editable")?;
    let interpreter = python.to_str().expect("invalid interpreter path");
    // The first argument is ignored by clap
    let mut cli = vec![
        "build",
        "--interpreter",
        &interpreter,
        "--manifest-path",
        &package_string,
        "--cargo-extra-args='--quiet'",
    ];

    if let Some(ref bindings) = bindings {
        cli.push("--bindings");
        cli.push(bindings);
    }

    let options: BuildOptions = BuildOptions::from_iter_safe(cli)?;

    let build_context = options.into_build_context(false, cfg!(feature = "faster-tests"), true)?;
    let wheels = build_context.build_wheels()?;

    for (filename, _supported_version) in wheels.iter() {
        // TODO: should add an assertion for .pth file in wheel root for mixed project layout
        let command = [
            "-m",
            "pip",
            "--disable-pip-version-check",
            "install",
            "--force-reinstall",
            &adjust_canonicalization(filename),
        ];
        let output = Command::new(&python)
            .args(&command)
            .output()
            .context(format!("pip install failed with {:?}", python))?;
        if !output.status.success() {
            bail!(
                "pip install in {} failed running {:?}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\n",
                venv_dir.display(),
                &command,
                output.status,
                str::from_utf8(&output.stdout)?.trim(),
                str::from_utf8(&output.stderr)?.trim(),
            );
        }
        if !output.stderr.is_empty() {
            bail!(
                "pip raised a warning running {:?}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\n",
                &command,
                output.status,
                str::from_utf8(&output.stdout)?.trim(),
                str::from_utf8(&output.stderr)?.trim(),
            );
        }

        check_installed(&package.as_ref(), &python)?;
    }

    Ok(())
}
