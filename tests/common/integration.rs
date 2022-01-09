use crate::common::{adjust_canonicalization, check_installed, maybe_mock_cargo};
use anyhow::{bail, Context, Result};
use clap::Parser;
use maturin::{BuildOptions, PythonInterpreter, Target};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str;

/// For each installed python version, this builds a wheel, creates a virtualenv if it
/// doesn't exist, installs the package and runs check_installed.py
pub fn test_integration(
    package: impl AsRef<Path>,
    bindings: Option<String>,
    unique_name: &str,
    zig: bool,
) -> Result<()> {
    maybe_mock_cargo();

    // Pass CARGO_BIN_EXE_maturin for testing purpose
    std::env::set_var("CARGO_BIN_EXE_maturin", env!("CARGO_BIN_EXE_maturin"));

    let target = Target::from_target_triple(None)?;

    let package_string = package.as_ref().join("Cargo.toml").display().to_string();

    // The first argument is ignored by clap
    let shed = format!("test-crates/wheels/{}", unique_name);
    let cargo_extra_args = format!(
        "--cargo-extra-args=--quiet --target-dir test-crates/targets/{}",
        unique_name
    );
    let mut cli = vec![
        "build",
        "--manifest-path",
        &package_string,
        &cargo_extra_args,
        "--out",
        &shed,
    ];

    if let Some(ref bindings) = bindings {
        cli.push("--bindings");
        cli.push(bindings);
    }

    if zig {
        cli.push("--zig")
    } else {
        cli.push("--compatibility");
        cli.push("linux");
    }

    let options: BuildOptions = BuildOptions::try_parse_from(cli)?;
    let build_context = options.into_build_context(false, cfg!(feature = "faster-tests"), false)?;
    let wheels = build_context.build_wheels()?;

    let test_name = package
        .as_ref()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    // For abi3 on unix, we didn't use a python interpreter, but we need one here
    let interpreter = if build_context.interpreter.is_empty() {
        let error_message = "python3 should be a python interpreter";
        let venv_interpreter = PythonInterpreter::check_executable(
            "python3",
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
        if zig && build_context.target.is_linux() && !build_context.target.is_musl_target() {
            assert!(filename
                .to_string_lossy()
                .ends_with("manylinux_2_12_x86_64.manylinux2010_x86_64.whl"))
        }
        let venv_name = if supported_version == "py3" {
            format!("{}-cffi", test_name)
        } else {
            format!(
                "{}-{}.{}",
                test_name, python_interpreter.major, python_interpreter.minor,
            )
        };
        let venv_dir = PathBuf::from("test-crates")
            .canonicalize()?
            .join("venvs")
            .join(venv_name);

        if !venv_dir.is_dir() {
            let output = Command::new("virtualenv")
                .arg("-p")
                .arg(python_interpreter.executable.clone())
                .arg(&adjust_canonicalization(&venv_dir))
                .output()?;

            if !output.status.success() {
                bail!(
                    "Failed to create a virtualenv at {}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}",
                    venv_dir.display(),
                    output.status,
                    str::from_utf8(&output.stdout)?,
                    str::from_utf8(&output.stderr)?,
                );
            }
        }

        let python = target.get_venv_python(&venv_dir);

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

        check_installed(package.as_ref(), &python)?;
    }

    Ok(())
}

/// Creates conda environments
#[cfg(target_os = "windows")]
fn create_conda_env(name: &str, major: usize, minor: usize) {
    Command::new("conda")
        .arg("create")
        .arg("-n")
        .arg(name)
        .arg(format!("python={}.{}", major, minor))
        .arg("-q")
        .arg("-y")
        .output()
        .expect("Conda not available.");
}

#[cfg(target_os = "windows")]
pub fn test_integration_conda(package: impl AsRef<Path>, bindings: Option<String>) -> Result<()> {
    use std::env;
    use std::process::Stdio;

    let package_string = package.as_ref().join("Cargo.toml").display().to_string();

    // Since the python launcher has precedence over conda, we need to deactivate it.
    // We do so by shadowing it with our own hello world binary.
    let original_path = env::var_os("PATH").expect("PATH is not defined");
    let py_dir = env::current_dir()?
        .join("test-data")
        .to_str()
        .unwrap()
        .to_string();
    let mocked_path = py_dir + ";" + original_path.to_str().unwrap();
    env::set_var("PATH", mocked_path);

    // Create environments to build against, prepended with "A" to ensure that integration
    // tests are executed with these environments
    create_conda_env("A-pyo3-build-env-36", 3, 6);
    create_conda_env("A-pyo3-build-env-37", 3, 7);
    create_conda_env("A-pyo3-build-env-38", 3, 8);
    create_conda_env("A-pyo3-build-env-39", 3, 9);

    // The first argument is ignored by clap
    let mut cli = vec![
        "build",
        "--manifest-path",
        &package_string,
        "--cargo-extra-args=--quiet",
    ];

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
        if executable.to_str().unwrap().contains("pyo3-build-env-") {
            conda_wheels.push((filename.clone(), executable))
        }
    }

    assert_eq!(
        3,
        conda_wheels.len(),
        "Error creating or detecting conda environments."
    );
    for (wheel_file, executable) in conda_wheels {
        let output = Command::new(&executable)
            .args(&[
                "-m",
                "pip",
                "--disable-pip-version-check",
                "install",
                "--force-reinstall",
                &adjust_canonicalization(wheel_file),
            ])
            .stderr(Stdio::inherit())
            .output()?;
        if !output.status.success() {
            panic!();
        }
        check_installed(&package.as_ref(), &executable)?;
    }

    Ok(())
}
