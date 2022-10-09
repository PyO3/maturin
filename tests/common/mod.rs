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

/// Check that the package is either not installed or works correctly
pub fn check_installed(package: &Path, python: &Path) -> Result<()> {
    let path = if cfg!(windows) {
        // on Windows, also add Scripts to PATH
        let python_dir = python.parent().unwrap();
        env::join_paths(&[&python_dir.join("Scripts"), python_dir])?.into()
    } else {
        python.parent().unwrap().to_path_buf()
    };
    let check_installed = Path::new(package)
        .join("check_installed")
        .join("check_installed.py");
    let output = Command::new(&python)
        .arg(check_installed)
        .env("PATH", path)
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
                eprintln!("Cause: {}", cause);
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

    let venv_dir = fs::canonicalize(PathBuf::from("test-crates"))?
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
        .arg(dunce::simplified(&venv_dir))
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

/// Creates conda environments
pub fn create_conda_env(name: &str, major: usize, minor: usize) -> Result<(PathBuf, PathBuf)> {
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct CondaCreateResult {
        prefix: PathBuf,
        success: bool,
    }

    let mut cmd = if cfg!(windows) {
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/c").arg("conda");
        cmd
    } else {
        Command::new("conda")
    };
    let output = cmd
        .arg("create")
        .arg("-n")
        .arg(name)
        .arg(format!("python={}.{}", major, minor))
        .arg("-q")
        .arg("-y")
        .arg("--json")
        .output()
        .expect("Conda not available.");
    let result: CondaCreateResult = serde_json::from_slice(&output.stdout)?;
    if !result.success {
        bail!("Failed to create conda environment {}.", name);
    }
    let target = Target::from_target_triple(None)?;
    let python = target.get_venv_python(&result.prefix);
    Ok((result.prefix, python))
}
