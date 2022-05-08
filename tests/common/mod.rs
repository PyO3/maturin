use anyhow::{bail, Result};
use fs_err as fs;
use maturin::Target;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{env, io, str};

pub mod develop;
pub mod editable;
pub mod errors;
pub mod integration;
pub mod other;

// Y U NO accept windows path prefix, pip?
// Anyways, here's shepmasters stack overflow solution
// https://stackoverflow.com/a/50323079/3549270
#[cfg(target_family = "unix")]
pub fn adjust_canonicalization(p: impl AsRef<Path>) -> String {
    p.as_ref().display().to_string()
}

#[cfg(target_os = "windows")]
pub fn adjust_canonicalization(p: impl AsRef<Path>) -> String {
    const VERBATIM_PREFIX: &str = r#"\\?\"#;
    let p = p.as_ref().display().to_string();
    if p.starts_with(VERBATIM_PREFIX) {
        p[VERBATIM_PREFIX.len()..].to_string()
    } else {
        p
    }
}

/// Check that the package is either not installed or works correctly
pub fn check_installed(package: &Path, python: &Path) -> Result<()> {
    let check_installed = Path::new(package)
        .join("check_installed")
        .join("check_installed.py");
    let output = Command::new(&python)
        .arg(check_installed)
        .env("PATH", python.parent().unwrap())
        .output()
        .unwrap();
    if !output.status.success() {
        bail!(
            "Check install fail: {} \n--- Stdout:\n{}\n--- Stderr:\n{}",
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    let message = str::from_utf8(&output.stdout).unwrap().trim();

    if message != "SUCCESS" {
        panic!("Not SUCCESS: {}", message);
    }

    Ok(())
}

/// Replaces the real cargo with cargo-mock if the mock crate has been compiled
///
/// If the mock crate hasn't been compile this does nothing
pub fn maybe_mock_cargo() {
    // libtest spawns multiple threads to run the tests in parallel, but all of those threads share
    // the same environment variables, so this uses the also global stdout lock to
    // make this region exclusive
    let stdout = io::stdout();
    let handle = stdout.lock();
    let mock_cargo_path = PathBuf::from("test-crates/cargo-mock/target/release/");
    if mock_cargo_path.join("cargo").is_file() || mock_cargo_path.join("cargo.exe").is_file() {
        let old_path = env::var_os("PATH").expect("PATH must be set");
        let mut path_split: Vec<PathBuf> = env::split_paths(&old_path).collect();
        // Another thread might have already modified the path
        if mock_cargo_path != path_split[0] {
            path_split.insert(0, mock_cargo_path);
            let new_path =
                env::join_paths(path_split).expect("Expected to be able to re-join PATH");
            env::set_var("PATH", new_path);
        }
    }
    drop(handle);
}

/// Better error formatting
pub fn handle_result<T>(result: Result<T>) -> T {
    match result {
        Err(e) => {
            for cause in e.chain().collect::<Vec<_>>().iter().rev() {
                eprintln!("{}", cause);
            }
            panic!("{}", e);
        }
        Ok(result) => result,
    }
}

/// Create virtualenv
pub fn create_virtualenv(
    package: impl AsRef<Path>,
    venv_suffix: &str,
    python_interp: Option<PathBuf>,
) -> Result<(PathBuf, PathBuf)> {
    let test_name = package
        .as_ref()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let venv_dir = PathBuf::from("test-crates")
        .canonicalize()?
        .join("venvs")
        .join(format!("{}-{}", test_name, venv_suffix));
    let target = Target::from_target_triple(None)?;

    if venv_dir.is_dir() {
        fs::remove_dir_all(&venv_dir)?;
    }

    let mut cmd = Command::new("virtualenv");
    if let Some(interp) = python_interp {
        cmd.arg("-p").arg(interp);
    }
    let output = cmd
        .arg(adjust_canonicalization(&venv_dir))
        .stderr(Stdio::inherit())
        .output()
        .expect("Failed to create a virtualenv");
    if !output.status.success() {
        panic!(
            "Failed to run virtualenv: {}\n---stdout:\n{}---stderr:\n{}",
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    let python = target.get_venv_python(&venv_dir);
    Ok((venv_dir, python))
}
