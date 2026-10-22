use anyhow::{Context, Result, bail};
use maturin::{DevelopOptions, Target, develop};
use std::env;
#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;
use tracing::{debug, instrument};

#[instrument(skip_all)]
pub fn develop_cmd(develop_options: DevelopOptions) -> Result<()> {
    let target = Target::from_target_triple(develop_options.cargo_options.target.as_ref())?;
    let venv_dir = detect_venv(&target)?;
    develop(develop_options, &venv_dir)?;
    Ok(())
}

#[cfg(any(windows, test))]
fn git_bash_path_to_windows(path: &Path) -> Option<PathBuf> {
    let path = path.to_str()?;
    let bytes = path.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'/' || !bytes[1].is_ascii_alphabetic() || bytes[2] != b'/' {
        return None;
    }

    let drive = (bytes[1] as char).to_ascii_uppercase();
    Some(PathBuf::from(format!("{drive}:{}", &path[2..])))
}

fn normalize_env_venv_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if !path.exists()
        && let Some(native_path) = git_bash_path_to_windows(&path)
        && native_path.exists()
    {
        debug!(
            original = %path.display(),
            normalized = %native_path.display(),
            "Normalized Git Bash virtualenv path"
        );
        return native_path;
    }

    path
}

fn detect_venv(target: &Target) -> Result<PathBuf> {
    let virtual_env = env::var_os("VIRTUAL_ENV")
        .map(PathBuf::from)
        .map(normalize_env_venv_path);
    let conda_prefix = env::var_os("CONDA_PREFIX")
        .map(PathBuf::from)
        .map(normalize_env_venv_path);

    match (virtual_env, conda_prefix) {
        (Some(dir), None) => return Ok(dir),
        (None, Some(dir)) => return Ok(dir),
        (Some(venv), Some(conda)) if venv == conda => return Ok(venv),
        (Some(_), Some(_)) => {
            bail!("Both VIRTUAL_ENV and CONDA_PREFIX are set. Please unset one of them")
        }
        (None, None) => {
            // No env var, try finding .venv
        }
    };

    let current_dir = env::current_dir().context("Failed to detect current directory ಠ_ಠ")?;
    // .venv in the current or any parent directory
    for dir in current_dir.ancestors() {
        let dot_venv = dir.join(".venv");
        if dot_venv.is_dir() {
            if !dot_venv.join("pyvenv.cfg").is_file() {
                bail!(
                    "Expected {} to be a virtual environment, but pyvenv.cfg is missing",
                    dot_venv.display()
                );
            }
            let python = target.get_venv_python(&dot_venv);
            if !python.is_file() {
                bail!(
                    "Your virtualenv at {} is broken. It contains a pyvenv.cfg but no python at {}",
                    dot_venv.display(),
                    python.display()
                );
            }
            debug!("Found a virtualenv named .venv at {}", dot_venv.display());
            return Ok(dot_venv);
        }
    }

    bail!(
        "Couldn't find a virtualenv or conda environment, but you need one to use this command. \
        For maturin to find your virtualenv you need to either set VIRTUAL_ENV (through activate), \
        set CONDA_PREFIX (through conda activate) or have a virtualenv called .venv in the current \
        or any parent folder. \
        See https://virtualenv.pypa.io/en/latest/index.html on how to use virtualenv or \
        use `maturin build` and `pip install <path/to/wheel>` instead."
    )
}

#[cfg(test)]
mod tests {
    use super::git_bash_path_to_windows;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_git_bash_path_to_windows() {
        assert_eq!(
            git_bash_path_to_windows(Path::new("/c/Users/me/project/.venv")),
            Some(PathBuf::from("C:/Users/me/project/.venv"))
        );
        assert_eq!(
            git_bash_path_to_windows(Path::new("/D/work/project/.venv")),
            Some(PathBuf::from("D:/work/project/.venv"))
        );
    }

    #[test]
    fn test_git_bash_path_to_windows_rejects_native_and_posix_paths() {
        assert_eq!(
            git_bash_path_to_windows(Path::new("C:/Users/me/project/.venv")),
            None
        );
        assert_eq!(git_bash_path_to_windows(Path::new("/usr/local/venv")), None);
        assert_eq!(git_bash_path_to_windows(Path::new("relative/.venv")), None);
    }
}
