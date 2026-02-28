use anyhow::{Context, Result, bail, ensure};
use fs_err as fs;
use regex::Regex;
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
        let re = match self {
            InstallBackend::Pip { .. } => Regex::new(r"pip ([\w\.]+).*"),
            InstallBackend::Uv { .. } => Regex::new(r"uv ([\w\.]+).*"),
        };
        match re.expect("regex should be valid").captures(stdout) {
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
                cmd.args(args).arg("pip");
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
