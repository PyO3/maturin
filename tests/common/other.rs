use anyhow::bail;
use anyhow::Result;
use maturin::BuildOptions;
use std::io::ErrorKind;
use std::process::Command;
use structopt::StructOpt;

/// Tries to compile a sample crate (pyo3-pure)  for musl,
/// given that rustup and the the musl target are installed
///
/// The bool in the Ok() response says whether the test was actually run
#[cfg(target_os = "linux")]
pub fn test_musl() -> Result<bool> {
    let get_target_list = Command::new("rustup")
        .args(&["target", "list", "--installed"])
        .output();

    match get_target_list {
        Ok(output) => {
            if output.status.success() {
                let has_musl = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.trim() == "x86_64-unknown-linux-musl");
                if !has_musl {
                    return Ok(false);
                }
            } else {
                bail!(
                    "`rustup target list --installed` failed with status {}",
                    output.status
                )
            }
        }
        // Ignore installations without rustup
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };

    // The first arg gets ignored
    let options: BuildOptions = BuildOptions::from_iter_safe(&[
        "build",
        "--manifest-path",
        "test-crates/pyo3-pure/Cargo.toml",
        "--interpreter",
        "python3",
        "--target",
        "x86_64-unknown-linux-musl",
        "--manylinux",
        "off",
    ])?;

    let build_context = options.into_build_context(false, cfg!(feature = "faster-tests"))?;
    let wheels = build_context.build_wheels()?;
    assert_eq!(wheels.len(), 1);

    Ok(true)
}

/// Test that we ignore non-existent Cargo.lock file listed by `cargo package --list`,
/// which seems to only occur with workspaces.
/// See https://github.com/rust-lang/cargo/issues/7938#issuecomment-593280660 and
/// https://github.com/PyO3/maturin/issues/449
pub fn test_workspace_cargo_lock() -> Result<()> {
    // The first arg gets ignored
    let options: BuildOptions = BuildOptions::from_iter_safe(&[
        "build",
        "--manifest-path",
        "test-crates/workspace/py/Cargo.toml",
        "--manylinux",
        "off",
    ])?;

    let build_context = options.into_build_context(false, cfg!(feature = "faster-tests"))?;
    let source_distribution = build_context.build_source_distribution()?;
    assert!(source_distribution.is_some());

    Ok(())
}
