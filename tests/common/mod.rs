use anyhow::{Result, bail};
use fs_err as fs;
use maturin::Target;
use normpath::PathExt as _;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{env, str};

pub mod develop;
pub mod errors;
pub mod integration;
pub mod other;
pub mod pep517;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TestInstallBackend {
    Pip,
    Uv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestEnvKind {
    Venv,
    Conda { major: usize, minor: usize },
}

pub struct PreparedEnv {
    pub root: PathBuf,
    pub python: PathBuf,
}

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn repo_test_crates_dir() -> PathBuf {
    repo_root().join("test-crates")
}

pub fn case_target_dir(case_id: &str) -> PathBuf {
    repo_test_crates_dir().join("targets").join(case_id)
}

pub fn case_wheel_dir(case_id: &str) -> PathBuf {
    repo_test_crates_dir().join("wheels").join(case_id)
}

pub fn is_ci() -> bool {
    env::var("GITHUB_ACTIONS").is_ok()
}

pub fn has_conda() -> bool {
    which::which("conda").is_ok()
}

pub fn has_uv() -> bool {
    which::which("uv").is_ok()
}

pub fn has_uniffi_bindgen() -> bool {
    which::which("uniffi-bindgen").is_ok()
}

/// Check that the package is either not installed or works correctly
pub fn check_installed(package: &Path, python: &Path) -> Result<()> {
    let path = if cfg!(windows) {
        // on Windows, also add Scripts to PATH
        let python_dir = python.parent().unwrap();
        env::join_paths([&python_dir.join("Scripts"), python_dir])?.into()
    } else {
        python.parent().unwrap().to_path_buf()
    };
    let mut check_installed = Path::new(package)
        .join("check_installed")
        .join("check_installed.py");
    if !check_installed.is_file() {
        check_installed = Path::new(package)
            .parent()
            .unwrap()
            .join("check_installed")
            .join("check_installed.py");
    }
    let output = Command::new(python)
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
        panic!("Not SUCCESS: {message}");
    }

    Ok(())
}

/// Better error formatting
#[track_caller]
pub fn handle_result<T>(result: Result<T>) -> T {
    match result {
        Err(e) => {
            for cause in e.chain().rev() {
                eprintln!("Cause: {cause}");
            }
            panic!("{}", e);
        }
        Ok(result) => result,
    }
}

/// Get Python implementation
pub fn get_python_implementation(python_interp: &Path) -> Result<String> {
    let code = "import sys; print(sys.implementation.name, end='')";
    let output = Command::new(python_interp).arg("-c").arg(code).output()?;
    let python_impl = String::from_utf8(output.stdout).unwrap();
    Ok(python_impl)
}

/// Get the current tested Python implementation
pub fn test_python_implementation() -> Result<String> {
    let python = test_python_path().map(PathBuf::from).unwrap_or_else(|| {
        let target = Target::from_target_triple(None).unwrap();
        target.get_python()
    });
    get_python_implementation(&python)
}

/// Create virtualenv
pub fn create_virtualenv(name: &str, python_interp: Option<PathBuf>) -> Result<(PathBuf, PathBuf)> {
    let interp = python_interp.or_else(|| test_python_path().map(PathBuf::from));
    let venv_interp = interp.clone().unwrap_or_else(|| {
        let target = Target::from_target_triple(None).unwrap();
        target.get_python()
    });
    let venv_name = match get_python_implementation(&venv_interp) {
        Ok(python_impl) => format!("{name}-{python_impl}"),
        Err(_) => name.to_string(),
    };

    let venv_dir = create_named_virtualenv(&venv_name, interp)?;

    let target = Target::from_target_triple(None)?;
    let python = target.get_venv_python(&venv_dir);
    Ok((venv_dir, python))
}

pub fn create_named_virtualenv(venv_name: &str, interp: Option<PathBuf>) -> Result<PathBuf> {
    let venv_dir = repo_test_crates_dir()
        .normalize()?
        .into_path_buf()
        .join("venvs")
        .join(venv_name);

    if venv_dir.is_dir() {
        fs::remove_dir_all(&venv_dir)?;
    }

    let mut cmd = {
        if let Ok(uv) = which::which("uv") {
            let mut cmd = Command::new(uv);
            cmd.args(["venv", "--seed"]);
            cmd
        } else {
            Command::new("virtualenv")
        }
    };
    if let Some(interp) = interp {
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
    Ok(venv_dir)
}

pub fn install_pip_packages(python: &Path, packages: &[&str]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    let output = Command::new(python)
        .args(["-m", "pip", "install", "--disable-pip-version-check"])
        .args(packages)
        .output()?;
    if !output.status.success() {
        bail!(
            "Failed to install {:?}: {}\n---stdout:\n{}---stderr:\n{}",
            packages,
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    Ok(())
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
        .arg(format!("python={major}.{minor}"))
        .arg("-q")
        .arg("-y")
        .arg("--json")
        .output()
        .expect("Conda not available.");
    if !output.status.success() {
        panic!(
            "Failed to create conda environment: {}\n---stdout:\n{}---stderr:\n{}",
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }
    let result: CondaCreateResult = serde_json::from_slice(&output.stdout)?;
    if !result.success {
        bail!("Failed to create conda environment {}.", name);
    }
    let target = Target::from_target_triple(None)?;
    let python = target.get_venv_python(&result.prefix);
    Ok((result.prefix, python))
}

pub fn prepare_test_env(
    case_id: &str,
    env_kind: TestEnvKind,
    prereq_packages: &[&str],
    python_interp: Option<PathBuf>,
) -> Result<PreparedEnv> {
    let (root, python) = match env_kind {
        TestEnvKind::Venv => create_virtualenv(case_id, python_interp)?,
        TestEnvKind::Conda { major, minor } => {
            create_conda_env(&format!("maturin-{case_id}"), major, minor)?
        }
    };
    install_pip_packages(&python, prereq_packages)?;
    Ok(PreparedEnv { root, python })
}

pub fn manifest_path_for_package(package: &Path) -> PathBuf {
    let pyproject_file = package.join("pyproject.toml");
    if pyproject_file.is_file()
        && let Ok(pyproject) = maturin::pyproject_toml::PyProjectToml::new(&pyproject_file)
        && let Some(manifest_path) = pyproject.manifest_path()
    {
        return package.join(manifest_path);
    }

    package.join("Cargo.toml")
}

/// Path to the python interpreter for testing
pub fn test_python_path() -> Option<String> {
    env::var("MATURIN_TEST_PYTHON").ok()
}
