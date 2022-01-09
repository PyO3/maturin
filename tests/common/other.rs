use anyhow::{Context, Result};
use clap::Parser;
use flate2::read::GzDecoder;
use maturin::BuildOptions;
use std::collections::HashSet;
use std::iter::FromIterator;
use std::path::Path;
use tar::Archive;

/// Tries to compile a sample crate (pyo3-pure) for musl,
/// given that rustup and the the musl target are installed
///
/// The bool in the Ok() response says whether the test was actually run
#[cfg(target_os = "linux")]
pub fn test_musl() -> Result<bool> {
    use anyhow::bail;
    use fs_err::File;
    use goblin::elf::Elf;
    use std::fs;
    use std::io::ErrorKind;
    use std::io::Read;
    use std::path::PathBuf;
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
    let options: BuildOptions = BuildOptions::try_parse_from(&[
        "build",
        "--manifest-path",
        "test-crates/hello-world/Cargo.toml",
        "--interpreter",
        "python3",
        "--target",
        "x86_64-unknown-linux-musl",
        "--compatibility",
        "linux",
        "--cargo-extra-args=--quiet --target-dir test-crates/targets/test_musl",
        "--out",
        "test-crates/wheels/test_musl",
    ])?;

    let build_context = options.into_build_context(false, cfg!(feature = "faster-tests"), false)?;
    let built_lib =
        PathBuf::from("test-crates/targets/test_musl/x86_64-unknown-linux-musl/debug/hello-world");
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
    let options: BuildOptions = BuildOptions::try_parse_from(&[
        "build",
        "--manifest-path",
        "test-crates/workspace/py/Cargo.toml",
        "--compatibility",
        "linux",
        "--cargo-extra-args=--quiet --target-dir test-crates/targets/test_workspace_cargo_lock",
        "--out",
        "test-crates/wheels/test_workspace_cargo_lock",
    ])?;

    let build_context = options.into_build_context(false, false, false)?;
    let source_distribution = build_context.build_source_distribution()?;
    assert!(source_distribution.is_some());

    Ok(())
}

pub fn test_source_distribution(
    package: impl AsRef<Path>,
    expected_files: Vec<&str>,
    unique_name: &str,
) -> Result<()> {
    let manifest_path = package.as_ref().join("Cargo.toml");
    let sdist_directory = Path::new("test-crates").join("wheels").join(unique_name);

    let build_options = BuildOptions {
        manifest_path,
        out: Some(sdist_directory),
        cargo_extra_args: vec![
            "--quiet".to_string(),
            "--target-dir".to_string(),
            "test-crates/targets/test_workspace_cargo_lock".to_string(),
        ],
        ..Default::default()
    };

    let build_context = build_options.into_build_context(false, false, false)?;
    let (path, _) = build_context
        .build_source_distribution()?
        .context("Failed to build source distribution")?;

    let tar_gz = fs_err::File::open(path)?;
    let tar = GzDecoder::new(tar_gz);
    let mut archive = Archive::new(tar);
    let mut files = HashSet::new();
    for entry in archive.entries()? {
        let entry = entry?;
        files.insert(format!("{}", entry.path()?.display()));
    }
    assert_eq!(
        files,
        HashSet::from_iter(expected_files.into_iter().map(ToString::to_string))
    );
    Ok(())
}
