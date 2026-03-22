use anyhow::{Context, Result, bail};
use maturin::{DevelopOptions, Target, develop};
use std::env;
use std::path::PathBuf;
use tracing::debug;

pub fn develop_cmd(develop_options: DevelopOptions) -> Result<()> {
    let target = Target::from_target_triple(develop_options.cargo_options.target.as_ref())?;
    let venv_dir = detect_venv(&target)?;
    develop(develop_options, &venv_dir)?;
    Ok(())
}

fn detect_venv(target: &Target) -> Result<PathBuf> {
    match (env::var_os("VIRTUAL_ENV"), env::var_os("CONDA_PREFIX")) {
        (Some(dir), None) => return Ok(PathBuf::from(dir)),
        (None, Some(dir)) => return Ok(PathBuf::from(dir)),
        (Some(venv), Some(conda)) if venv == conda => return Ok(PathBuf::from(venv)),
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
