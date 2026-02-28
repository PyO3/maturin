use crate::common::{check_installed, create_conda_env, create_virtualenv, maybe_mock_cargo};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::str;

pub fn target_dir(unique_name: &str) -> String {
    format!(
        "{}/test-crates/targets/{unique_name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// pip install to install it and checks it is working
pub fn test_pep517(
    package: impl AsRef<Path>,
    unique_name: &str,
    conda: bool,
    editable: bool,
) -> Result<Output> {
    maybe_mock_cargo();

    let package = package.as_ref();
    let (_venv_dir, python) = if conda {
        create_conda_env(&format!("maturin-{unique_name}"), 3, 10)?
    } else {
        create_virtualenv(unique_name, None)?
    };

    // Ensure the test doesn't wrongly pass
    check_installed(package, &python).unwrap_err();

    // Install `tomli` into the virtualenv (runtime dependency of `maturin`'s pep517 builds for
    // Python <3.11)
    let mut cmd = Command::new(&python);
    cmd.args(["-m", "pip", "install", "tomli"]);
    let output = cmd.output()?;
    if !output.status.success() {
        panic!(
            "Failed to install tomli: {}\n---stdout:\n{}---stderr:\n{}",
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    let mut cmd = Command::new(&python);
    cmd.args(["-m", "pip", "install", "-vvv", "--no-build-isolation"]);

    if editable {
        cmd.arg("--editable");
    }

    cmd.arg(package.to_str().expect("package is utf8 path"));

    let target_dir = target_dir(unique_name);
    cmd.env("CARGO_TARGET_DIR", target_dir);

    // Building with `--no-build-isolation` means that `maturin` needs to be on PATH _and_
    // importable

    // Hack PATH to include maturin binary directory
    let maturin_exe = Path::new(env!("CARGO_BIN_EXE_maturin"));
    let bin_dir = maturin_exe.parent();
    cmd.env("PATH", insert_path("PATH", bin_dir.unwrap()));

    // Hack PYTHONPATH to include the root of the repository
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    cmd.env("PYTHONPATH", insert_path("PYTHONPATH", repo_root));

    let output = cmd.output()?;
    if !output.status.success() {
        panic!(
            "Failed to install {}: {}\n---stdout:\n{}---stderr:\n{}",
            package.display(),
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }

    check_installed(package, &python)?;
    Ok(output)
}

fn insert_path(env_var: &str, new_path: &Path) -> String {
    let old_path = std::env::var_os(env_var).unwrap_or_default();
    let mut paths = std::env::split_paths(&old_path).collect::<Vec<PathBuf>>();
    paths.insert(0, new_path.to_path_buf());
    std::env::join_paths(paths)
        .expect("Expected to be able to re-join PATH")
        .into_string()
        .expect("PATH is not valid utf8")
}

/// Whether cargo built for the specified cargo profile in the test target directory.
pub fn target_has_profile(unique_name: &str, profile: &str) -> bool {
    let profile_dir = PathBuf::from(target_dir(unique_name)).join(profile);
    // Check for cargo's .fingerprint directory which is always created for the
    // profile that was used, and is not affected by maturin's artifact staging.
    profile_dir.join(".fingerprint").is_dir()
}
