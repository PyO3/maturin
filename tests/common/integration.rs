use crate::common::{check_installed, create_virtualenv, maybe_mock_cargo, test_python_path};
use anyhow::{bail, Context, Result};
use cargo_zigbuild::Zig;
use clap::Parser;
use maturin::{BuildOptions, PythonInterpreter};
use std::env;
use std::path::Path;
use std::process::Command;
use std::str;

/// For each installed python version, this builds a wheel, creates a virtualenv if it
/// doesn't exist, installs the package and runs check_installed.py
pub fn test_integration(
    package: impl AsRef<Path>,
    bindings: Option<String>,
    unique_name: &str,
    zig: bool,
    target: Option<&str>,
) -> Result<()> {
    maybe_mock_cargo();

    // Pass CARGO_BIN_EXE_maturin for testing purpose
    env::set_var(
        "CARGO_BIN_EXE_cargo-zigbuild",
        env!("CARGO_BIN_EXE_maturin"),
    );

    let package_string = package.as_ref().join("Cargo.toml").display().to_string();

    // The first argument is ignored by clap
    let shed = format!("test-crates/wheels/{}", unique_name);
    let target_dir = format!("test-crates/targets/{}", unique_name);
    let python_interp = test_python_path();
    let mut cli = vec![
        "build",
        "--quiet",
        "--manifest-path",
        &package_string,
        "--target-dir",
        &target_dir,
        "--out",
        &shed,
    ];

    if let Some(ref bindings) = bindings {
        cli.push("--bindings");
        cli.push(bindings);
    }

    if let Some(target) = target {
        cli.push("--target");
        cli.push(target)
    }

    let test_zig = if zig && (env::var("GITHUB_ACTIONS").is_ok() || Zig::find_zig().is_ok()) {
        cli.push("--zig");
        true
    } else {
        cli.push("--compatibility");
        cli.push("linux");
        false
    };

    if let Some(interp) = python_interp.as_ref() {
        cli.push("--interpreter");
        cli.push(interp);
    }

    let options: BuildOptions = BuildOptions::try_parse_from(cli)?;
    let build_context = options.into_build_context(false, cfg!(feature = "faster-tests"), false)?;
    let wheels = build_context.build_wheels()?;

    // For abi3 on unix, we didn't use a python interpreter, but we need one here
    let interpreter = if build_context.interpreter.is_empty() {
        let error_message = "python3 should be a python interpreter";
        let venv_interpreter = PythonInterpreter::check_executable(
            python_interp.as_deref().unwrap_or("python3"),
            &build_context.target,
            &build_context.bridge,
        )
        .context(error_message)?
        .context(error_message)?;
        vec![venv_interpreter]
    } else {
        build_context.interpreter
    };
    // We can do this since we know that wheels are built and returned in the
    // order they are in the build context
    for ((filename, supported_version), python_interpreter) in wheels.iter().zip(interpreter) {
        if test_zig && build_context.target.is_linux() && !build_context.target.is_musl_target() {
            let rustc_ver = rustc_version::version()?;
            let file_suffix = if rustc_ver >= semver::Version::new(1, 64, 0) {
                "manylinux_2_17_x86_64.manylinux2014_x86_64.whl"
            } else {
                "manylinux_2_12_x86_64.manylinux2010_x86_64.whl"
            };
            assert!(filename.to_string_lossy().ends_with(file_suffix))
        }
        let mut venv_name = if supported_version == "py3" {
            format!("{}-py3", unique_name)
        } else {
            format!(
                "{}-py{}.{}",
                unique_name, python_interpreter.major, python_interpreter.minor,
            )
        };
        if let Some(target) = target {
            venv_name = format!("{}-{}", venv_name, target);
        }
        let (venv_dir, python) =
            create_virtualenv(&venv_name, Some(python_interpreter.executable.clone()))?;

        let command = [
            "-m",
            "pip",
            "--disable-pip-version-check",
            "--no-cache-dir",
            "install",
            "--force-reinstall",
        ];
        let output = Command::new(&python)
            .args(command)
            .arg(dunce::simplified(filename))
            .output()
            .context(format!("pip install failed with {:?}", python))?;
        if !output.status.success() {
            let full_command = format!("{} {}", python.display(), command.join(" "));
            bail!(
                "pip install in {} failed running {:?}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\n",
                venv_dir.display(),
                full_command,
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

        check_installed(package.as_ref(), &python)?;
    }

    Ok(())
}

pub fn test_integration_conda(package: impl AsRef<Path>, bindings: Option<String>) -> Result<()> {
    use crate::common::create_conda_env;
    use std::path::PathBuf;
    use std::process::Stdio;

    let package_string = package.as_ref().join("Cargo.toml").display().to_string();

    // Create environments to build against, prepended with "A" to ensure that integration
    // tests are executed with these environments
    let mut interpreters = Vec::new();
    for minor in 7..=10 {
        let (_, venv_python) = create_conda_env(&format!("A-maturin-env-3{}", minor), 3, minor)?;
        interpreters.push(venv_python);
    }

    // The first argument is ignored by clap
    let mut cli = vec![
        "build",
        "--manifest-path",
        &package_string,
        "--quiet",
        "--interpreter",
    ];
    for interp in &interpreters {
        cli.push(interp.to_str().unwrap());
    }

    if let Some(ref bindings) = bindings {
        cli.push("--bindings");
        cli.push(bindings);
    }

    let options = BuildOptions::try_parse_from(cli)?;

    let build_context = options.into_build_context(false, cfg!(feature = "faster-tests"), false)?;
    let wheels = build_context.build_wheels()?;

    let mut conda_wheels: Vec<(PathBuf, PathBuf)> = vec![];
    for ((filename, _), python_interpreter) in wheels.iter().zip(build_context.interpreter) {
        let executable = python_interpreter.executable;
        if executable.to_str().unwrap().contains("maturin-env-") {
            conda_wheels.push((filename.clone(), executable))
        }
    }

    assert_eq!(
        interpreters.len(),
        conda_wheels.len(),
        "Error creating or detecting conda environments."
    );
    for (wheel_file, executable) in conda_wheels {
        let output = Command::new(&executable)
            .args([
                "-m",
                "pip",
                "--disable-pip-version-check",
                "install",
                "--force-reinstall",
            ])
            .arg(dunce::simplified(&wheel_file))
            .stderr(Stdio::inherit())
            .output()?;
        if !output.status.success() {
            panic!();
        }
        check_installed(package.as_ref(), &executable)?;
    }

    Ok(())
}
