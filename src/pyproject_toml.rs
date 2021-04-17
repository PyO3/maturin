use anyhow::{format_err, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// The `[build-system]` section of a pyproject.toml as specified in PEP 517
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct BuildSystem {
    requires: Vec<String>,
    build_backend: String,
}

/// The `[tool]` section of a pyproject.toml
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Tool {
    maturin: Option<ToolMaturin>,
}

/// The `[tool.maturin]` section of a pyproject.toml
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct ToolMaturin {
    sdist_include: Option<Vec<String>>,
}

/// A pyproject.toml as specified in PEP 517
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PyProjectToml {
    build_system: BuildSystem,
    tool: Option<Tool>,
}

impl PyProjectToml {
    /// Returns the contents of a pyproject.toml with a `[build-system]` entry or an error
    ///
    /// Does no specific error handling because it's only used to check whether or not to build
    /// source distributions
    pub fn new(project_root: impl AsRef<Path>) -> Result<PyProjectToml> {
        let path = project_root.as_ref().join("pyproject.toml");
        let contents = fs::read_to_string(&path).context(format!(
            "Couldn't find pyproject.toml at {}",
            path.display()
        ))?;
        let cargo_toml = toml::from_str(&contents)
            .map_err(|err| format_err!("pyproject.toml is not PEP 517 compliant: {}", err))?;
        Ok(cargo_toml)
    }

    /// Returns the value of `[maturin.sdist-include]` in pyproject.toml
    pub fn sdist_include(&self) -> Option<&Vec<String>> {
        self.tool.as_ref()?.maturin.as_ref()?.sdist_include.as_ref()
    }
}
