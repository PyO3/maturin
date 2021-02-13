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
    let result = options.into_build_context(false, cfg!(feature = "faster-tests"));
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
#[cfg(target_os = "linux")]
pub fn pyo3_no_extension_module() -> Result<()> {
    use anyhow::format_err;

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
        .into_build_context(false, cfg!(feature = "faster-tests"))?
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
