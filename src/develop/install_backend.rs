use anyhow::{Context, Result, bail, ensure};
use fs_err as fs;
use once_cell::sync::Lazy;
use regex::Regex;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str;
use tracing::debug;

pub(crate) enum InstallBackend {
    Pip {
        path: Option<PathBuf>,
    },
    Uv {
        path: PathBuf,
        args: Vec<&'static str>,
    },
}

impl InstallBackend {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            InstallBackend::Pip { .. } => "pip",
            InstallBackend::Uv { .. } => "uv pip",
        }
    }

    pub(crate) fn version(&self, python_path: &Path) -> Result<semver::Version> {
        let mut cmd = self.make_command(python_path);

        // Newer versions of uv no longer support `uv pip --version`, and instead
        // require that we use `uv --version`. This is a workaround to get the
        // version of the install backend for both old and new versions of uv.
        cmd = match self {
            InstallBackend::Pip { .. } => cmd,
            InstallBackend::Uv { path, args } => {
                let mut cmd = Command::new(path);
                cmd.args(args);
                cmd
            }
        };
        let output = cmd
            .arg("--version")
            .output()
            .context("failed to get version of install backend")?;
        ensure!(
            output.status.success(),
            "failed to get version of install backend"
        );
        let stdout = str::from_utf8(&output.stdout)?;
        static PIP_VERSION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"pip ([\w\.]+).*").unwrap());
        static UV_VERSION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"uv ([\w\.]+).*").unwrap());
        let re = match self {
            InstallBackend::Pip { .. } => &*PIP_VERSION_RE,
            InstallBackend::Uv { .. } => &*UV_VERSION_RE,
        };
        match re.captures(stdout) {
            Some(captures) => Ok(semver::Version::parse(&captures[1])
                .with_context(|| format!("failed to parse semver from {stdout:?}"))?),
            _ => {
                bail!("failed to parse version from {:?}", stdout);
            }
        }
    }

    /// check whether this install backend supports `show --files`. Returns Ok(()) if it does.
    pub(crate) fn check_supports_show_files(&self, python_path: &Path) -> Result<()> {
        match self {
            InstallBackend::Pip { .. } => Ok(()),
            InstallBackend::Uv { .. } => {
                // https://github.com/astral-sh/uv/releases/tag/0.4.25
                let version = self.version(python_path)?;
                if version < semver::Version::new(0, 4, 25) {
                    bail!(
                        "uv >= 0.4.25 is required for `show --files`. Version {} was found.",
                        version
                    );
                }
                Ok(())
            }
        }
    }

    pub(crate) fn stderr_indicates_problem(&self) -> bool {
        match self {
            InstallBackend::Pip { .. } => true,
            // `uv pip install` sends regular logs to stderr, not just errors
            InstallBackend::Uv { .. } => false,
        }
    }

    pub(crate) fn make_command(&self, python_path: &Path) -> Command {
        match self {
            InstallBackend::Pip { path } => match &path {
                Some(path) => {
                    let mut cmd = Command::new(path);
                    cmd.arg("--python")
                        .arg(python_path)
                        .arg("--disable-pip-version-check");
                    cmd
                }
                None => {
                    let mut cmd = Command::new(python_path);
                    cmd.arg("-m").arg("pip").arg("--disable-pip-version-check");
                    cmd
                }
            },
            InstallBackend::Uv { path, args } => {
                let mut cmd = Command::new(path);
                cmd.args(args).arg("pip").env("UV_PYTHON", python_path);
                cmd
            }
        }
    }
}

/// Detect the plain uv binary
pub(crate) fn find_uv_bin() -> Result<(PathBuf, Vec<&'static str>)> {
    let output = Command::new("uv").arg("--version").output()?;
    if output.status.success() {
        let version_str =
            str::from_utf8(&output.stdout).context("`uv --version` didn't return utf8 output")?;
        debug!(version = %version_str, "Found uv binary in PATH");
        Ok((PathBuf::from("uv"), Vec::new()))
    } else {
        bail!("`uv --version` failed with status: {}", output.status);
    }
}

fn venv_search_path(venv_dir: &Path, search_path: &std::ffi::OsStr) -> Result<std::ffi::OsString> {
    let paths = env::split_paths(search_path)
        .filter(|path| path.starts_with(venv_dir))
        .collect::<Vec<_>>();
    ensure!(
        !paths.is_empty(),
        "No PATH entries belong to virtual environment {}",
        venv_dir.display()
    );
    env::join_paths(paths).context("Failed to construct virtual environment PATH")
}

fn find_executable_in_path(executable: &str, search_path: &std::ffi::OsStr) -> Option<PathBuf> {
    let executable = format!("{}{}", executable, env::consts::EXE_SUFFIX);
    env::split_paths(search_path)
        .map(|directory| directory.join(&executable))
        .find(|path| path.is_file())
}

/// Detect a uv binary that belongs to the active virtual environment.
///
/// This intentionally ignores uv executables found elsewhere on PATH so a
/// globally installed uv cannot hijack a pixi environment that should use pip.
pub(crate) fn find_uv_bin_in_venv(venv_dir: &Path) -> Result<(PathBuf, Vec<&'static str>)> {
    let path = env::var_os("PATH").context("PATH is not set")?;
    let search_path = venv_search_path(venv_dir, &path)?;
    let uv_path = find_executable_in_path("uv", &search_path)
        .context("uv is not installed in the active virtual environment")?;
    let output = Command::new(&uv_path).arg("--version").output()?;
    if output.status.success() {
        let version_str =
            str::from_utf8(&output.stdout).context("`uv --version` didn't return utf8 output")?;
        debug!(path = %uv_path.display(), version = %version_str, "Found uv binary in virtual environment");
        Ok((uv_path, Vec::new()))
    } else {
        bail!(
            "`{} --version` failed with status: {}",
            uv_path.display(),
            output.status
        );
    }
}

/// Detect the Python uv package
pub(crate) fn find_uv_python(python_path: &Path) -> Result<(PathBuf, Vec<&'static str>)> {
    let output = Command::new(python_path)
        .args(["-m", "uv", "--version"])
        .output()?;
    if output.status.success() {
        let version_str =
            str::from_utf8(&output.stdout).context("`uv --version` didn't return utf8 output")?;
        debug!(version = %version_str, "Found Python uv module");
        Ok((python_path.to_path_buf(), vec!["-m", "uv"]))
    } else {
        bail!(
            "`{} -m uv --version` failed with status: {}",
            python_path.display(),
            output.status
        );
    }
}

pub(crate) fn check_pip_exists(python_path: &Path, pip_path: Option<&PathBuf>) -> Result<()> {
    let output = if let Some(pip_path) = pip_path {
        Command::new(pip_path).args(["--version"]).output()?
    } else {
        Command::new(python_path)
            .args(["-m", "pip", "--version"])
            .output()?
    };
    if output.status.success() {
        let version_str =
            str::from_utf8(&output.stdout).context("`pip --version` didn't return utf8 output")?;
        debug!(version = %version_str, "Found pip");
        Ok(())
    } else {
        bail!("`pip --version` failed with status: {}", output.status);
    }
}

/// Check if a virtualenv is created by uv by reading pyvenv.cfg
pub(crate) fn is_uv_venv(venv_dir: &Path) -> bool {
    let pyvenv_cfg = venv_dir.join("pyvenv.cfg");
    if !pyvenv_cfg.exists() {
        return false;
    }
    match fs::read_to_string(&pyvenv_cfg) {
        Ok(content) => content.contains("\nuv = "),
        Err(_) => false,
    }
}

/// Check if a virtualenv is created by pixi by checking for `conda-meta/pixi_env_prefix`
pub(crate) fn is_pixi_venv(venv_dir: &Path) -> bool {
    venv_dir.join("conda-meta").join("pixi_env_prefix").exists()
}

#[cfg(test)]
mod tests {
    use super::{find_executable_in_path, is_pixi_venv, venv_search_path};
    use fs_err as fs;
    use std::env;
    use tempfile::TempDir;

    #[test]
    fn test_find_executable_in_path_uses_filtered_path() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let executable = bin_dir.join(format!("uv{}", env::consts::EXE_SUFFIX));
        fs::write(&executable, "").unwrap();
        let search_path = env::join_paths([&bin_dir]).unwrap();

        assert_eq!(
            find_executable_in_path("uv", &search_path),
            Some(executable)
        );
    }

    #[test]
    fn test_venv_search_path_excludes_global_entries() {
        let tmp = TempDir::new().unwrap();
        let venv_bin = tmp.path().join("bin");
        let global_bin = tmp.path().parent().unwrap().join("global-bin");
        let search_path = env::join_paths([&venv_bin, &global_bin]).unwrap();

        let filtered = venv_search_path(tmp.path(), &search_path).unwrap();
        let paths = env::split_paths(&filtered).collect::<Vec<_>>();

        assert_eq!(paths, vec![venv_bin]);
    }

    #[test]
    fn test_venv_search_path_rejects_only_global_entries() {
        let tmp = TempDir::new().unwrap();
        let global_bin = tmp.path().parent().unwrap().join("global-bin");
        let search_path = env::join_paths([&global_bin]).unwrap();

        assert!(venv_search_path(tmp.path(), &search_path).is_err());
    }

    #[test]
    fn test_is_pixi_venv_detects_marker() {
        let tmp = TempDir::new().unwrap();
        let conda_meta = tmp.path().join("conda-meta");
        fs::create_dir_all(&conda_meta).unwrap();
        fs::write(conda_meta.join("pixi_env_prefix"), "").unwrap();
        assert!(is_pixi_venv(tmp.path()));
    }

    #[test]
    fn test_is_pixi_venv_rejects_plain_venv() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        assert!(!is_pixi_venv(tmp.path()));
    }

    #[test]
    fn test_is_pixi_venv_rejects_vanilla_conda() {
        let tmp = TempDir::new().unwrap();
        let conda_meta = tmp.path().join("conda-meta");
        fs::create_dir_all(&conda_meta).unwrap();
        fs::write(conda_meta.join("python-3.12.0-h12345.json"), "{}").unwrap();
        assert!(!is_pixi_venv(tmp.path()));
    }
}
