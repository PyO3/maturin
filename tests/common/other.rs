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
    let options = BuildOptions::from_iter_safe(&[
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
