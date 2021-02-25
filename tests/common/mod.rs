use anyhow::{bail, Result};
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::{env, io, str};

pub mod develop;
pub mod errors;
pub mod integration;
#[cfg(target_os = "linux")]
pub mod other;

// Y U NO accept windows path prefix, pip?
// Anyways, here's shepmasters stack overflow solution
// https://stackoverflow.com/a/50323079/3549270
#[cfg(not(target_os = "windows"))]
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
pub fn check_installed(package: &Path, python: &PathBuf) -> Result<()> {
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
        panic!("{}", message);
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
        // Another thread might have aready modified the path
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
