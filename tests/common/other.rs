use anyhow::Result;
use maturin::BuildOptions;
use structopt::StructOpt;

/// Tries to compile a sample crate (pyo3-pure) for musl,
/// given that rustup and the the musl target are installed
///
/// The bool in the Ok() response says whether the test was actually run
#[cfg(target_os = "linux")]
pub fn test_musl() -> Result<bool> {
    use anyhow::{bail, format_err};
    use fs_err::File;
    use goblin::elf::Elf;
    use std::fs;
    use std::io::ErrorKind;
    use std::io::Read;
    use std::process::Command;

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
        "test-crates/hello-world/Cargo.toml",
        "--interpreter",
        "python3",
        "--target",
        "x86_64-unknown-linux-musl",
        "--compatibility",
        "linux",
    ])?;

    let build_context = options.into_build_context(false, cfg!(feature = "faster-tests"), false)?;
    let built_lib = build_context
        .manifest_path
        .parent()
        .ok_or(format_err!("Missing parent directory"))?
        .join("target/x86_64-unknown-linux-musl/debug/hello-world");
    if built_lib.is_file() {
        fs::remove_file(&built_lib)?;
    }
    let wheels = build_context.build_wheels()?;
    assert_eq!(wheels.len(), 1);

    // Ensure that we've actually built for musl
    assert!(built_lib.is_file());
    let mut file = File::open(built_lib)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let elf = Elf::parse(&buffer)?;
    assert_eq!(elf.libraries, Vec::<&str>::new());

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
        "--compatibility",
        "linux",
    ])?;

    let build_context = options.into_build_context(false, cfg!(feature = "faster-tests"), false)?;
    let source_distribution = build_context.build_source_distribution()?;
    assert!(source_distribution.is_some());

    Ok(())
}
