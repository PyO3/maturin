use crate::PlatformTag;
use anyhow::{format_err, Context, Result};
use pyproject_toml::PyProjectToml as ProjectToml;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

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
    bindings: Option<String>,
    cargo_extra_args: Option<String>,
    #[serde(alias = "manylinux")]
    compatibility: Option<PlatformTag>,
    rustc_extra_args: Option<String>,
    #[serde(default)]
    skip_auditwheel: bool,
    #[serde(default)]
    strip: bool,
}

/// A pyproject.toml as specified in PEP 517
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PyProjectToml {
    #[serde(flatten)]
    inner: ProjectToml,
    /// PEP 518: The `[tool]` table is where any tool related to your Python project, not just build
    /// tools, can have users specify configuration data as long as they use a sub-table within
    /// `[tool]`, e.g. the flit tool would store its configuration in `[tool.flit]`.
    ///
    /// We use it for `[tool.maturin]`
    pub tool: Option<Tool>,
}

impl std::ops::Deref for PyProjectToml {
    type Target = ProjectToml;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
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
        let pyproject: PyProjectToml = toml::from_str(&contents)
            .map_err(|err| format_err!("pyproject.toml is not PEP 517 compliant: {}", err))?;
        Ok(pyproject)
    }

    /// Returns the value of `[tool.maturin.sdist-include]` in pyproject.toml
    pub fn sdist_include(&self) -> Option<&Vec<String>> {
        self.tool.as_ref()?.maturin.as_ref()?.sdist_include.as_ref()
    }

    /// Returns the value of `[tool.maturin.bindings]` in pyproject.toml
    pub fn bindings(&self) -> Option<&str> {
        self.tool.as_ref()?.maturin.as_ref()?.bindings.as_deref()
    }

    /// Returns the value of `[tool.maturin.cargo-extra-args]` in pyproject.toml
    pub fn cargo_extra_args(&self) -> Option<&str> {
        self.tool
            .as_ref()?
            .maturin
            .as_ref()?
            .cargo_extra_args
            .as_deref()
    }

    /// Returns the value of `[tool.maturin.compatibility]` in pyproject.toml
    pub fn compatibility(&self) -> Option<PlatformTag> {
        self.tool.as_ref()?.maturin.as_ref()?.compatibility
    }

    /// Returns the value of `[tool.maturin.rustc-extra-args]` in pyproject.toml
    pub fn rustc_extra_args(&self) -> Option<&str> {
        self.tool
            .as_ref()?
            .maturin
            .as_ref()?
            .rustc_extra_args
            .as_deref()
    }

    /// Returns the value of `[tool.maturin.skip-auditwheel]` in pyproject.toml
    pub fn skip_auditwheel(&self) -> bool {
        self.tool
            .as_ref()
            .and_then(|tool| tool.maturin.as_ref())
            .map(|maturin| maturin.skip_auditwheel)
            .unwrap_or_default()
    }

    /// Returns the value of `[tool.maturin.strip]` in pyproject.toml
    pub fn strip(&self) -> bool {
        self.tool
            .as_ref()
            .and_then(|tool| tool.maturin.as_ref())
            .map(|maturin| maturin.strip)
            .unwrap_or_default()
    }

    /// Having a pyproject.toml without a version constraint is a bad idea
    /// because at some point we'll have to do breaking changes and then source
    /// distributions would break
    ///
    /// Returns true if the pyproject.toml has the constraint
    pub fn warn_missing_maturin_version(&self) -> bool {
        let maturin = env!("CARGO_PKG_NAME");
        if let Some(requires_maturin) = self
            .build_system
            .requires
            .iter()
            .find(|x| x.starts_with(maturin))
        {
            // Note: Update this once 1.0 is out
            assert_eq!(env!("CARGO_PKG_VERSION_MAJOR"), "0");
            let current_minor: usize = env!("CARGO_PKG_VERSION_MINOR").parse().unwrap();
            if requires_maturin == maturin {
                eprintln!(
                    "⚠️  Warning: Please use {maturin} in pyproject.toml with a version constraint, \
                    e.g. `requires = [\"{maturin}>=0.{current},<0.{next}\"]`. \
                    This will become an error.",
                    maturin = maturin,
                    current = current_minor,
                    next = current_minor + 1,
                );
                return false;
            }
        }
        true
    }

    /// Having a pyproject.toml without `build-backend` set to `maturin`
    /// may result in build errors when build from source distribution
    ///
    /// Returns true if the pyproject.toml has `build-backend` set to `maturin`
    pub fn warn_missing_build_backend(&self) -> bool {
        let maturin = env!("CARGO_PKG_NAME");
        if self.build_system.build_backend.as_deref() != Some(maturin) {
            eprintln!(
                "⚠️  Warning: `build-backend` in pyproject.toml is not set to `{maturin}`, \
                    packaging tools such as pip will not use maturin to build this project.",
                maturin = maturin
            );
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::PyProjectToml;
    use fs_err as fs;
    use tempfile::TempDir;

    #[test]
    fn test_warn_missing_maturin_version() {
        let with_constraint = PyProjectToml::new("test-crates/pyo3-pure").unwrap();
        assert!(with_constraint.warn_missing_maturin_version());
        let without_constraint_dir = TempDir::new().unwrap();

        fs::write(
            without_constraint_dir.path().join("pyproject.toml"),
            r#"[build-system]
            requires = ["maturin"]
            build-backend = "maturin"

            [tool.maturin]
            manylinux = "2010""#,
        )
        .unwrap();
        let without_constraint = PyProjectToml::new(without_constraint_dir).unwrap();
        assert!(!without_constraint.warn_missing_maturin_version());
    }
}
