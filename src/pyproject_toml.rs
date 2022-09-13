use crate::PlatformTag;
use anyhow::{format_err, Result};
use fs_err as fs;
use pyproject_toml::PyProjectToml as ProjectToml;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    // maturin specific options
    sdist_include: Option<Vec<String>>,
    bindings: Option<String>,
    #[serde(alias = "manylinux")]
    compatibility: Option<PlatformTag>,
    #[serde(default)]
    skip_auditwheel: bool,
    #[serde(default)]
    strip: bool,
    /// The directory with python module, contains `<module_name>/__init__.py`
    python_source: Option<PathBuf>,
    /// Path to the wheel directory, defaults to `<module_name>.data`
    data: Option<PathBuf>,
    // Some customizable cargo options
    /// Build artifacts with the specified Cargo profile
    pub profile: Option<String>,
    /// Space or comma separated list of features to activate
    pub features: Option<Vec<String>>,
    /// Activate all available features
    pub all_features: Option<bool>,
    /// Do not activate the `default` feature
    pub no_default_features: Option<bool>,
    /// Path to Cargo.toml
    pub manifest_path: Option<PathBuf>,
    /// Require Cargo.lock and cache are up to date
    pub frozen: Option<bool>,
    /// Require Cargo.lock is up to date
    pub locked: Option<bool>,
    /// Override a configuration value (unstable)
    pub config: Option<Vec<String>>,
    /// Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details
    pub unstable_flags: Option<Vec<String>>,
    pub rustc_args: Option<Vec<String>>,
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
    pub fn new(pyproject_file: impl AsRef<Path>) -> Result<PyProjectToml> {
        let path = pyproject_file.as_ref();
        let contents = fs::read_to_string(&path)?;
        let pyproject: PyProjectToml = toml_edit::easy::from_str(&contents)
            .map_err(|err| format_err!("pyproject.toml is not PEP 517 compliant: {}", err))?;
        Ok(pyproject)
    }

    /// Returns the values of `[tool.maturin]` in pyproject.toml
    #[inline]
    pub fn maturin(&self) -> Option<&ToolMaturin> {
        self.tool.as_ref()?.maturin.as_ref()
    }

    /// Returns the value of `[tool.maturin.sdist-include]` in pyproject.toml
    pub fn sdist_include(&self) -> Option<&Vec<String>> {
        self.maturin()?.sdist_include.as_ref()
    }

    /// Returns the value of `[tool.maturin.bindings]` in pyproject.toml
    pub fn bindings(&self) -> Option<&str> {
        self.maturin()?.bindings.as_deref()
    }

    /// Returns the value of `[tool.maturin.compatibility]` in pyproject.toml
    pub fn compatibility(&self) -> Option<PlatformTag> {
        self.maturin()?.compatibility
    }

    /// Returns the value of `[tool.maturin.skip-auditwheel]` in pyproject.toml
    pub fn skip_auditwheel(&self) -> bool {
        self.maturin()
            .map(|maturin| maturin.skip_auditwheel)
            .unwrap_or_default()
    }

    /// Returns the value of `[tool.maturin.strip]` in pyproject.toml
    pub fn strip(&self) -> bool {
        self.maturin()
            .map(|maturin| maturin.strip)
            .unwrap_or_default()
    }

    /// Returns the value of `[tool.maturin.python-source]` in pyproject.toml
    pub fn python_source(&self) -> Option<&Path> {
        self.maturin()
            .and_then(|maturin| maturin.python_source.as_deref())
    }

    /// Returns the value of `[tool.maturin.data]` in pyproject.toml
    pub fn data(&self) -> Option<&Path> {
        self.maturin().and_then(|maturin| maturin.data.as_deref())
    }

    /// Returns the value of `[tool.maturin.manifest-path]` in pyproject.toml
    pub fn manifest_path(&self) -> Option<&Path> {
        self.maturin()?.manifest_path.as_deref()
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
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn test_parse_tool_maturin() {
        let tmp_dir = TempDir::new().unwrap();
        let pyproject_file = tmp_dir.path().join("pyproject.toml");

        fs::write(
            &pyproject_file,
            r#"[build-system]
            requires = ["maturin"]
            build-backend = "maturin"

            [tool.maturin]
            manylinux = "2010"
            manifest-path = "Cargo.toml"
            profile = "dev"
            features = ["foo", "bar"]
            no-default-features = true
            locked = true
            rustc-args = ["-Z", "unstable-options"]
            "#,
        )
        .unwrap();
        let pyproject = PyProjectToml::new(pyproject_file).unwrap();
        assert_eq!(pyproject.manifest_path(), Some(Path::new("Cargo.toml")));

        let maturin = pyproject.maturin().unwrap();
        assert_eq!(maturin.profile.as_deref(), Some("dev"));
        assert_eq!(
            maturin.features,
            Some(vec!["foo".to_string(), "bar".to_string()])
        );
        assert!(maturin.all_features.is_none());
        assert_eq!(maturin.no_default_features, Some(true));
        assert_eq!(maturin.locked, Some(true));
        assert!(maturin.frozen.is_none());
        assert_eq!(
            maturin.rustc_args,
            Some(vec!["-Z".to_string(), "unstable-options".to_string()])
        );
    }

    #[test]
    fn test_warn_missing_maturin_version() {
        let with_constraint = PyProjectToml::new("test-crates/pyo3-pure/pyproject.toml").unwrap();
        assert!(with_constraint.warn_missing_maturin_version());
        let without_constraint_dir = TempDir::new().unwrap();
        let pyproject_file = without_constraint_dir.path().join("pyproject.toml");

        fs::write(
            &pyproject_file,
            r#"[build-system]
            requires = ["maturin"]
            build-backend = "maturin"

            [tool.maturin]
            manylinux = "2010"
            "#,
        )
        .unwrap();
        let without_constraint = PyProjectToml::new(pyproject_file).unwrap();
        assert!(!without_constraint.warn_missing_maturin_version());
    }
}
