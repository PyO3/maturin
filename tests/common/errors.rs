use anyhow::format_err;
use anyhow::{bail, Result};
use clap::Parser;
use maturin::BuildOptions;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::process::Command;
use std::str;

pub fn abi3_without_version() -> Result<()> {
    // The first argument is ignored by clap
    let cli = vec![
        "build",
        "--manifest-path",
        "test-crates/pyo3-abi3-without-version/Cargo.toml",
        "--quiet",
        "--target-dir",
        "test-targets/wheels/abi3_without_version",
    ];

    let options = BuildOptions::try_parse_from(cli)?;
    let result = options
        .into_build_context()
        .release(false)
        .strip(cfg!(feature = "faster-tests"))
        .editable(false)
        .build();
    match result {
        Err(err) => {
            assert_eq!(err.to_string(),
            "You have selected the `abi3` feature but not a minimum version (e.g. the `abi3-py36` feature). \
            maturin needs a minimum version feature to build abi3 wheels."
        );
        }
        _ => {
            bail!("Should have errored");
        }
    }

    Ok(())
}

/// Check that you get a good error message if you forgot to set the extension-module feature
pub fn pyo3_no_extension_module() -> Result<()> {
    // The first argument is ignored by clap
    let cli = vec![
        "build",
        "--manifest-path",
        "test-crates/pyo3-no-extension-module/Cargo.toml",
        "--quiet",
        "--target-dir",
        "test-crates/targets/pyo3_no_extension_module",
        "--out",
        "test-crates/targets/pyo3_no_extension_module",
    ];

    let options = BuildOptions::try_parse_from(cli)?;
    let result = options
        .into_build_context()
        .release(false)
        .strip(cfg!(feature = "faster-tests"))
        .editable(false)
        .build()?
        .build_wheels();
    match result {
        Err(err) => {
            if !(err
                .source()
                .ok_or_else(|| format_err!("{}", err))?
                .to_string()
                .starts_with("Your library links libpython"))
            {
                return Err(err);
            }
        }
        _ => {
            bail!("Should have errored");
        }
    }

    Ok(())
}

/// Make sure cargo metadata doesn't create a lock file when --locked was passed
///
/// https://github.com/PyO3/maturin/issues/472
pub fn locked_doesnt_build_without_cargo_lock() -> Result<()> {
    // The first argument is ignored by clap
    let cli = vec![
        "build",
        "--manifest-path",
        "test-crates/lib_with_path_dep/Cargo.toml",
        "--locked",
        "-itargetspython",
        "--target-dir",
        "test-crates/targets/locked_doesnt_build_without_cargo_lock",
    ];
    let options = BuildOptions::try_parse_from(cli)?;
    let result = options
        .into_build_context()
        .release(false)
        .strip(cfg!(feature = "faster-tests"))
        .editable(false)
        .build();
    match result {
        Err(err) => {
            let err_string = err
                .source()
                .ok_or_else(|| format_err!("{}", err))?
                .to_string();
            if !err_string.starts_with("`cargo metadata` exited with an error:") {
                bail!("{:?}", err_string);
            }
        }
        _ => {
            bail!("Should have errored");
        }
    }

    Ok(())
}

/// Don't panic if the manylinux version doesn't exit
///
/// https://github.com/PyO3/maturin/issues/739
pub fn invalid_manylinux_does_not_panic() -> Result<()> {
    // The first argument is ignored by clap
    let cli = vec![
        "build",
        "-m",
        "test-crates/pyo3-mixed/Cargo.toml",
        "--compatibility",
        "manylinux_2_99",
        "--target-dir",
        "test-crates/targets/invalid_manylinux_does_not_panic",
        "--out",
        "test-crates/targets/invalid_manylinux_does_not_panic",
    ];
    let options: BuildOptions = BuildOptions::try_parse_from(cli)?;
    let result = options
        .into_build_context()
        .release(false)
        .strip(cfg!(feature = "faster-tests"))
        .editable(false)
        .build()?
        .build_wheels();
    match result {
        Err(err) => {
            assert_eq!(err.to_string(), "Error ensuring manylinux_2_99 compliance");
            let err_string = err
                .source()
                .ok_or_else(|| format_err!("{}", err))?
                .to_string();
            assert_eq!(err_string, "manylinux_2_99 compatibility policy is not defined by auditwheel yet, pass `--auditwheel=skip` to proceed anyway");
        }
        _ => {
            bail!("Should have errored");
        }
    }

    Ok(())
}

/// The user set `python-source` in pyproject.toml, but there is no python module in there
pub fn warn_on_missing_python_source() -> Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_maturin"))
        .arg("build")
        .arg("-m")
        .arg(
            Path::new("test-crates")
                .join("wrong-python-source")
                .join("Cargo.toml"),
        )
        .output()
        .unwrap();
    if !output.status.success() {
        bail!(
            "Failed to run: {}\n---stdout:\n{}---stderr:\n{}",
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    assert!(str::from_utf8(&output.stderr)?.contains("Warning: You specified the python source as"));
    Ok(())
}
