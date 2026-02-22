use crate::common::{
    check_installed, create_named_virtualenv, create_virtualenv, maybe_mock_cargo, test_python_path,
};
use anyhow::{Context, Result, bail};
#[cfg(feature = "zig")]
use cargo_zigbuild::Zig;
use clap::Parser;
use fs_err::File;
use fs4::fs_err3::FileExt;
use maturin::{BuildOptions, PlatformTag, PythonInterpreter, Target};
use normpath::PathExt;
use std::collections::HashSet;
use std::env;
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
    target: Option<&str>,
) -> Result<()> {
    maybe_mock_cargo();

    // Pass CARGO_BIN_EXE_maturin for testing purpose
    unsafe {
        env::set_var(
            "CARGO_BIN_EXE_cargo-zigbuild",
            env!("CARGO_BIN_EXE_maturin"),
        )
    };

    let package_string = package.as_ref().join("Cargo.toml").display().to_string();

    // The first argument is ignored by clap
    let shed = format!("test-crates/wheels/{unique_name}");
    let target_dir = format!("test-crates/targets/{unique_name}");
    let python_interp = test_python_path();
    let mut cli: Vec<std::ffi::OsString> = vec![
        "build".into(),
        "--quiet".into(),
        "--manifest-path".into(),
        package_string.into(),
        "--target-dir".into(),
        target_dir.into(),
        "--out".into(),
        shed.into(),
    ];

    if let Some(ref bindings) = bindings {
        cli.push("--bindings".into());
        cli.push(bindings.into());
    }

    if let Some(target) = target {
        cli.push("--target".into());
        cli.push(target.into())
    }

    #[cfg(feature = "zig")]
    let zig_found = Zig::find_zig().is_ok();
    #[cfg(not(feature = "zig"))]
    let zig_found = false;

    let test_zig = if zig && (env::var("GITHUB_ACTIONS").is_ok() || zig_found) {
        cli.push("--zig".into());
        true
    } else {
        cli.push("--compatibility".into());
        cli.push("linux".into());
        false
    };

    // One scope up to extend the lifetime
    let venvs_dir = Path::new("test-crates")
        .normalize()?
        .into_path_buf()
        .join("venvs");
    fs_err::create_dir_all(&venvs_dir)?;
    let cffi_provider = "cffi-provider";
    let cffi_venv = venvs_dir.join(cffi_provider);

    // on PyPy, we should use the bundled cffi
    if let Some(interp) = python_interp
        .as_ref()
        .filter(|interp| interp.contains("pypy"))
    {
        cli.push("--interpreter".into());
        cli.push(interp.into());
    } else {
        // Install cffi in a separate environment

        // All tests try to use this venv at the same time, so we need to make sure only one
        // modifies it at a time and that during that time, no other test reads it.
        let file = File::create(venvs_dir.join("cffi-provider.lock"))?;
        file.lock_exclusive()?;
        let python = if !cffi_venv.is_dir() {
            create_named_virtualenv(cffi_provider, python_interp.clone().map(PathBuf::from))?;
            let target_triple = Target::from_target_triple(None)?;
            let python = target_triple.get_venv_python(&cffi_venv);
            assert!(python.is_file(), "cffi venv not created correctly");
            let pip_install_cffi = [
                "-m",
                "pip",
                "--disable-pip-version-check",
                "--no-cache-dir",
                "install",
                "cffi",
            ];
            let output = Command::new(&python)
                .args(pip_install_cffi)
                .output()
                .with_context(|| format!("pip install cffi failed with {python:?}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                bail!(
                    "Installing cffi into {} failed.\nstdout: {}\nstderr: {}",
                    cffi_venv.display(),
                    stdout,
                    stderr
                );
            }
            python
        } else {
            let target_triple = Target::from_target_triple(None)?;
            target_triple.get_venv_python(&cffi_venv)
        };
        file.unlock()?;
        cli.push("--interpreter".into());
        cli.push(python.as_os_str().to_owned());
    }

    let options: BuildOptions = BuildOptions::try_parse_from(cli)?;
    let build_context = options
        .into_build_context()
        .strip(Some(cfg!(feature = "faster-tests")))
        .editable(false)
        .build()?;
    let wheels = build_context.build_wheels()?;

    // For abi3 on unix, we didn't use a python interpreter, but we need one here
    let interpreter = if build_context.interpreter.is_empty() {
        let error_message = "python3 should be a python interpreter";
        let venv_interpreter = maturin::python_interpreter::check_executable(
            python_interp.as_deref().unwrap_or("python3"),
            &build_context.target,
            build_context.bridge(),
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
        check_for_duplicates(filename)?;
        if test_zig
            && build_context.target.is_linux()
            && !build_context.target.is_musl_libc()
            && build_context.target.get_minimum_manylinux_tag() != PlatformTag::Linux
        {
            let rustc_ver = rustc_version::version()?;
            let python_arch = build_context.target.get_python_arch();
            let file_suffix = if rustc_ver >= semver::Version::new(1, 64, 0) {
                format!("manylinux_2_17_{python_arch}.manylinux2014_{python_arch}.whl")
            } else {
                format!("manylinux_2_12_{python_arch}.manylinux2010_{python_arch}.whl")
            };
            assert!(filename.to_string_lossy().ends_with(&file_suffix))
        }
        let mut venv_name = if supported_version == "py3" {
            format!("{unique_name}-py3")
        } else {
            format!(
                "{}-py{}.{}",
                unique_name, python_interpreter.major, python_interpreter.minor,
            )
        };
        if let Some(target) = target {
            venv_name = format!("{venv_name}-{target}");
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
            .context(format!("pip install failed with {python:?}"))?;
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
    for minor in 9..=12 {
        let (_, venv_python) = create_conda_env(&format!("A-maturin-env-3{minor}"), 3, minor)?;
        interpreters.push(venv_python);
    }

    // The first argument is ignored by clap
    let mut cli: Vec<std::ffi::OsString> = vec![
        "build".into(),
        "--manifest-path".into(),
        package_string.into(),
        "--quiet".into(),
        "--interpreter".into(),
    ];
    for interp in &interpreters {
        cli.push(interp.to_str().unwrap().into());
    }

    if let Some(ref bindings) = bindings {
        cli.push("--bindings".into());
        cli.push(bindings.into());
    }

    let options = BuildOptions::try_parse_from(cli)?;

    let build_context = options
        .into_build_context()
        .strip(Some(cfg!(feature = "faster-tests")))
        .editable(false)
        .build()?;
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

/// See <https://github.com/PyO3/maturin/issues/2106> and
/// <https://github.com/PyO3/maturin/issues/2066>.
fn check_for_duplicates(wheel: &Path) -> Result<()> {
    let mut seen = HashSet::new();
    let mut reader = File::open(wheel)?;
    // We have to use this API since `ZipArchive` deduplicates names.
    while let Some(file) = zip::read::read_zipfile_from_stream(&mut reader)? {
        if !seen.insert(file.name().to_string()) {
            bail!("Duplicate file: {}", file.name());
        }
    }
    Ok(())
}
