use anyhow::format_err;
use anyhow::{bail, Result};
use maturin::BuildOptions;
use structopt::StructOpt;

pub fn abi3_without_version() -> Result<()> {
    // The first argument is ignored by clap
    let cli = vec![
        "build",
        "--manifest-path",
        "test-crates/pyo3-abi3-without-version/Cargo.toml",
        "--cargo-extra-args='--quiet'",
    ];

    let options = BuildOptions::from_iter_safe(cli)?;
    let result = options.into_build_context(false, cfg!(feature = "faster-tests"), false);
    if let Err(err) = result {
        assert_eq!(err.to_string(),
            "You have selected the `abi3` feature but not a minimum version (e.g. the `abi3-py36` feature). \
            maturin needs a minimum version feature to build abi3 wheels."
        );
    } else {
        bail!("Should have errored");
    }

    Ok(())
}

/// Check that you get a good error message if you forgot to set the extension-module feature
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn pyo3_no_extension_module() -> Result<()> {
    // The first argument is ignored by clap
    let cli = vec![
        "build",
        "--manifest-path",
        "test-crates/pyo3-no-extension-module/Cargo.toml",
        "--cargo-extra-args='--quiet'",
        "-i=python",
    ];

    let options = BuildOptions::from_iter_safe(cli)?;
    let result = options
        .into_build_context(false, cfg!(feature = "faster-tests"), false)?
        .build_wheels();
    if let Err(err) = result {
        if !(err
            .source()
            .ok_or_else(|| format_err!("{}", err))?
            .to_string()
            .starts_with("Your library links libpython"))
        {
            return Err(err);
        }
    } else {
        bail!("Should have errored");
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
        "--cargo-extra-args='--locked'",
        "-i=python",
    ];
    let options: BuildOptions = BuildOptions::from_iter_safe(cli)?;
    let result = options.into_build_context(false, cfg!(feature = "faster-tests"), false);
    if let Err(err) = result {
        let err_string = err
            .source()
            .ok_or_else(|| format_err!("{}", err))?
            .to_string();
        // Sometimes the first message is "waiting for file lock on package cache",
        // so we can only check if the lock file is in the message somewhere
        if !err_string.starts_with("`cargo metadata` exited with an error:")
            || !err_string.contains("lock file")
        {
            bail!("{:?}", err_string);
        }
    } else {
        bail!("Should have errored");
    }

    Ok(())
}

/// Don't panic if the manylinux version doesn't exit
///
/// https://github.com/PyO3/maturin/issues/739
pub fn invalid_manylinux_doesnt_panic() -> Result<()> {
    // The first argument is ignored by clap
    let cli = vec![
        "build",
        "-m",
        "test-crates/pyo3-mixed/Cargo.toml",
        "-i=python",
        "--compatibility",
        "manylinux_2_99",
    ];
    let options: BuildOptions = BuildOptions::from_iter_safe(cli)?;
    let result = options
        .into_build_context(false, cfg!(feature = "faster-tests"), false)?
        .build_wheels();
    if let Err(err) = result {
        assert_eq!(err.to_string(), "Error ensuring manylinux_2_99 compliance");
        let err_string = err
            .source()
            .ok_or_else(|| format_err!("{}", err))?
            .to_string();
        assert_eq!(err_string, "manylinux_2_99 compatibility policy is not defined by auditwheel yet, pass `--skip-auditwheel` to proceed anyway");
    } else {
        bail!("Should have errored");
    }

    Ok(())
}
