//! A pyproject.toml as specified in PEP 517

use crate::PlatformTag;
use crate::auditwheel::AuditWheelMode;
use anyhow::{Context, Result};
use fs_err as fs;
use pep440_rs::Version;
use pep508_rs::VersionOrUrl;
use pyproject_toml::{BuildSystem, Project};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The `[tool]` section of a pyproject.toml
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "kebab-case")]
pub struct Tool {
    /// maturin options
    pub maturin: Option<ToolMaturin>,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
/// The target format for the include or exclude [GlobPattern].
///
/// See [Formats].
pub enum Format {
    /// Source distribution
    Sdist,
    /// Wheel
    Wheel,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
/// A single [Format] or multiple [Format] values for a [GlobPattern].
pub enum Formats {
    /// A single [Format] value
    Single(Format),
    /// Multiple [Format] values
    Multiple(Vec<Format>),
}

impl Formats {
    /// Returns `true` if the inner [Format] value(s) target the given [Format]
    pub fn targets(&self, format: Format) -> bool {
        match self {
            Self::Single(val) if val == &format => true,
            Self::Multiple(formats) if formats.contains(&format) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
/// A glob pattern for the include and exclude configuration.
///
/// See [PyProjectToml::include] and [PyProject::exclude].
///
/// Based on <https://python-poetry.org/docs/pyproject/#include-and-exclude>.
pub enum GlobPattern {
    /// A glob
    Path(String),
    /// A glob `path` with a `format` key to specify one or more [Format] values
    WithFormat {
        /// A glob
        path: String,
        /// One or more [Format] values
        format: Formats,
    },
}

impl GlobPattern {
    /// Returns the glob pattern for this pattern if it targets the given [Format], else this returns `None`.
    pub fn targets(&self, format: Format) -> Option<&str> {
        match self {
            // Not specified defaults to both
            Self::Path(glob) => Some(glob),
            Self::WithFormat {
                path,
                format: formats,
            } if formats.targets(format) => Some(path),
            _ => None,
        }
    }
}

/// Cargo compile target
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CargoTarget {
    /// Name as given in the `Cargo.toml` or generated from the file name
    pub name: String,
    /// Kind of target ("bin", "cdylib")
    pub kind: Option<CargoCrateType>,
    // TODO: Add bindings option
    // Bridge model, which kind of bindings to use
    // pub bindings: Option<String>,
}

/// Supported cargo crate types
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum CargoCrateType {
    /// Binary executable target
    #[serde(rename = "bin")]
    Bin,
    /// Dynamic system library target
    #[serde(rename = "cdylib")]
    CDyLib,
    /// Dynamic Rust library target
    #[serde(rename = "dylib")]
    DyLib,
    /// Rust library
    #[serde(rename = "lib")]
    Lib,
    /// Rust library for use as an intermediate target
    #[serde(rename = "rlib")]
    RLib,
    /// Static library
    #[serde(rename = "staticlib")]
    StaticLib,
}

impl From<CargoCrateType> for cargo_metadata::CrateType {
    fn from(value: CargoCrateType) -> Self {
        match value {
            CargoCrateType::Bin => cargo_metadata::CrateType::Bin,
            CargoCrateType::CDyLib => cargo_metadata::CrateType::CDyLib,
            CargoCrateType::DyLib => cargo_metadata::CrateType::DyLib,
            CargoCrateType::Lib => cargo_metadata::CrateType::Lib,
            CargoCrateType::RLib => cargo_metadata::CrateType::RLib,
            CargoCrateType::StaticLib => cargo_metadata::CrateType::StaticLib,
        }
    }
}

/// Target configuration
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TargetConfig {
    /// macOS deployment target version
    #[serde(alias = "macosx-deployment-target")]
    pub macos_deployment_target: Option<String>,
}

/// Source distribution generator
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum SdistGenerator {
    /// Use `cargo package --list`
    #[default]
    Cargo,
    /// Use `git ls-files`
    Git,
}

/// The `[tool.maturin]` section of a pyproject.toml
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ToolMaturin {
    // maturin specific options
    /// Module name, accepts setuptools style import name like `foo.bar`
    pub module_name: Option<String>,
    /// Include files matching the given glob pattern(s)
    pub include: Option<Vec<GlobPattern>>,
    /// Exclude files matching the given glob pattern(s)
    pub exclude: Option<Vec<GlobPattern>>,
    /// Bindings type
    pub bindings: Option<String>,
    /// Platform compatibility
    #[serde(alias = "manylinux")]
    pub compatibility: Option<PlatformTag>,
    /// Audit wheel mode
    pub auditwheel: Option<AuditWheelMode>,
    /// Skip audit wheel
    #[serde(default)]
    pub skip_auditwheel: bool,
    /// Strip the final binary
    #[serde(default)]
    pub strip: bool,
    /// Source distribution generator
    #[serde(default)]
    pub sdist_generator: SdistGenerator,
    /// The directory with python module, contains `<module_name>/__init__.py`
    pub python_source: Option<PathBuf>,
    /// Python packages to include
    pub python_packages: Option<Vec<String>>,
    /// Path to the wheel directory, defaults to `<module_name>.data`
    pub data: Option<PathBuf>,
    /// Cargo compile targets
    pub targets: Option<Vec<CargoTarget>>,
    /// Target configuration
    #[serde(default, rename = "target")]
    pub target_config: HashMap<String, TargetConfig>,
    // Some customizable cargo options
    /// Build artifacts with the specified Cargo profile
    pub profile: Option<String>,
    /// Same as `profile` but for "editable" builds
    pub editable_profile: Option<String>,
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
    /// Additional rustc arguments
    pub rustc_args: Option<Vec<String>>,
    /// Use base Python executable instead of venv Python executable in PEP 517 build.
    //
    // This can help avoid unnecessary rebuilds, as the Python executable does not change
    // every time. It should not be set when the sdist build requires packages installed
    // in venv.
    #[serde(default)]
    pub use_base_python: bool,
}

/// A pyproject.toml as specified in PEP 517
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PyProjectToml {
    /// Build-related data
    pub build_system: BuildSystem,
    /// Project metadata
    pub project: Option<Project>,
    /// PEP 518: The `[tool]` table is where any tool related to your Python project, not just build
    /// tools, can have users specify configuration data as long as they use a sub-table within
    /// `[tool]`, e.g. the flit tool would store its configuration in `[tool.flit]`.
    ///
    /// We use it for `[tool.maturin]`
    pub tool: Option<Tool>,
}

impl PyProjectToml {
    /// Returns the contents of a pyproject.toml with a `[build-system]` entry or an error
    ///
    /// Does no specific error handling because it's only used to check whether or not to build
    /// source distributions
    pub fn new(pyproject_file: impl AsRef<Path>) -> Result<PyProjectToml> {
        let path = pyproject_file.as_ref();
        let contents = fs::read_to_string(path)?;
        let pyproject = toml::from_str(&contents).with_context(|| {
            format!(
                "pyproject.toml at {} is invalid",
                pyproject_file.as_ref().display()
            )
        })?;
        Ok(pyproject)
    }

    /// Returns the value of `[project.name]` in pyproject.toml
    pub fn project_name(&self) -> Option<&str> {
        self.project.as_ref().map(|project| project.name.as_str())
    }

    /// Returns the values of `[tool.maturin]` in pyproject.toml
    #[inline]
    pub fn maturin(&self) -> Option<&ToolMaturin> {
        self.tool.as_ref()?.maturin.as_ref()
    }

    /// Returns the value of `[tool.maturin.module-name]` in pyproject.toml
    pub fn module_name(&self) -> Option<&str> {
        self.maturin()?.module_name.as_deref()
    }

    /// Returns the value of `[tool.maturin.include]` in pyproject.toml
    pub fn include(&self) -> Option<&[GlobPattern]> {
        self.maturin()?.include.as_ref().map(AsRef::as_ref)
    }

    /// Returns the value of `[tool.maturin.exclude]` in pyproject.toml
    pub fn exclude(&self) -> Option<&[GlobPattern]> {
        self.maturin()?.exclude.as_ref().map(AsRef::as_ref)
    }

    /// Returns the value of `[tool.maturin.bindings]` in pyproject.toml
    pub fn bindings(&self) -> Option<&str> {
        self.maturin()?.bindings.as_deref()
    }

    /// Returns the value of `[tool.maturin.compatibility]` in pyproject.toml
    pub fn compatibility(&self) -> Option<PlatformTag> {
        self.maturin()?.compatibility
    }

    /// Returns the value of `[tool.maturin.auditwheel]` in pyproject.toml
    pub fn auditwheel(&self) -> Option<AuditWheelMode> {
        self.maturin()
            .map(|maturin| maturin.auditwheel)
            .unwrap_or_default()
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

    /// Returns the value of `[tool.maturin.sdist-generator]` in pyproject.toml
    pub fn sdist_generator(&self) -> SdistGenerator {
        self.maturin()
            .map(|maturin| maturin.sdist_generator)
            .unwrap_or_default()
    }

    /// Returns the value of `[tool.maturin.python-source]` in pyproject.toml
    pub fn python_source(&self) -> Option<&Path> {
        self.maturin()
            .and_then(|maturin| maturin.python_source.as_deref())
    }

    /// Returns the value of `[tool.maturin.python-packages]` in pyproject.toml
    pub fn python_packages(&self) -> Option<&[String]> {
        self.maturin()
            .and_then(|maturin| maturin.python_packages.as_deref())
    }

    /// Returns the value of `[tool.maturin.data]` in pyproject.toml
    pub fn data(&self) -> Option<&Path> {
        self.maturin().and_then(|maturin| maturin.data.as_deref())
    }

    /// Returns the value of `[tool.maturin.targets]` in pyproject.toml
    pub fn targets(&self) -> Option<Vec<CargoTarget>> {
        self.maturin().and_then(|maturin| maturin.targets.clone())
    }

    /// Returns the value of `[tool.maturin.target.<target>]` in pyproject.toml
    pub fn target_config(&self, target: &str) -> Option<&TargetConfig> {
        self.maturin()
            .and_then(|maturin| maturin.target_config.get(target))
    }

    /// Returns the value of `[tool.maturin.manifest-path]` in pyproject.toml
    pub fn manifest_path(&self) -> Option<&Path> {
        self.maturin()?.manifest_path.as_deref()
    }

    /// Warn about `build-system.requires` mismatching expectations.
    ///
    /// Having a pyproject.toml without a version constraint is a bad idea
    /// because at some point we'll have to do breaking changes and then source
    /// distributions would break.
    ///
    /// The second problem we check for is the current maturin version not matching the constraint.
    ///
    /// Returns false if a warning was emitted.
    pub fn warn_bad_maturin_version(&self) -> bool {
        let maturin = env!("CARGO_PKG_NAME");
        let current_major = env!("CARGO_PKG_VERSION_MAJOR").parse::<usize>().unwrap();
        let self_version = Version::from_str(env!("CARGO_PKG_VERSION")).unwrap();
        let requires_maturin = self
            .build_system
            .requires
            .iter()
            .find(|x| x.name.as_ref() == maturin);
        if let Some(requires_maturin) = requires_maturin {
            match requires_maturin.version_or_url.as_ref() {
                Some(VersionOrUrl::VersionSpecifier(version_specifier)) => {
                    if !version_specifier.contains(&self_version) {
                        eprintln!(
                            "⚠️  Warning: You specified {requires_maturin} in pyproject.toml under \
                            `build-system.requires`, but the current {maturin} version is {self_version}",
                        );
                        return false;
                    }
                }
                Some(VersionOrUrl::Url(_)) => {
                    // We can't check this
                }
                None => {
                    eprintln!(
                        "⚠️  Warning: Please use {maturin} in pyproject.toml with a version constraint, \
                        e.g. `requires = [\"{maturin}>={current}.0,<{next}.0\"]`. \
                        This will become an error.",
                        maturin = maturin,
                        current = current_major,
                        next = current_major + 1,
                    );
                    return false;
                }
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
        if self.build_system.build_backend.as_deref() == Some(maturin) {
            return true;
        }

        if std::env::var("MATURIN_NO_MISSING_BUILD_BACKEND_WARNING").is_ok() {
            return false;
        }

        eprintln!(
            "⚠️  Warning: `build-backend` in pyproject.toml is not set to `{maturin}`, \
                packaging tools such as pip will not use maturin to build this project."
        );
        false
    }

    /// Having a pyproject.toml project table with neither `version` nor `dynamic = ['version']`
    /// violates https://packaging.python.org/en/latest/specifications/pyproject-toml/#dynamic.
    ///
    /// Returns true if version information is specified correctly or no project table is present.
    pub fn warn_invalid_version_info(&self) -> bool {
        let Some(project) = &self.project else {
            return true;
        };
        let has_static_version = project.version.is_some();
        let has_dynamic_version = project
            .dynamic
            .as_ref()
            .is_some_and(|d| d.iter().any(|s| s == "version"));
        if has_static_version && has_dynamic_version {
            eprintln!(
                "⚠️  Warning: `project.dynamic` must not specify `version` when `project.version` is present in pyproject.toml"
            );
            return false;
        }
        if !has_static_version && !has_dynamic_version {
            eprintln!(
                "⚠️  Warning: `project.version` field is required in pyproject.toml unless it is present in the `project.dynamic` list"
            );
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        PyProjectToml,
        pyproject_toml::{Format, Formats, GlobPattern, ToolMaturin},
    };
    use expect_test::expect;
    use fs_err as fs;
    use indoc::indoc;
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
            python-packages = ["foo", "bar"]
            manifest-path = "Cargo.toml"
            profile = "dev"
            features = ["foo", "bar"]
            no-default-features = true
            locked = true
            rustc-args = ["-Z", "unstable-options"]

            [[tool.maturin.targets]]
            name = "pyo3_pure"
            kind = "lib"
            bindings = "pyo3"

            [tool.maturin.target."x86_64-apple-darwin"]
            macos-deployment-target = "10.12"
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
        assert_eq!(
            maturin.python_packages,
            Some(vec!["foo".to_string(), "bar".to_string()])
        );
        let targets = maturin.targets.as_ref().unwrap();
        assert_eq!("pyo3_pure", targets[0].name);
        let target_config = pyproject.target_config("x86_64-apple-darwin").unwrap();
        assert_eq!(
            target_config.macos_deployment_target.as_deref(),
            Some("10.12")
        );
    }

    #[test]
    fn test_warn_missing_maturin_version() {
        let with_constraint = PyProjectToml::new("test-crates/pyo3-pure/pyproject.toml").unwrap();
        assert!(with_constraint.warn_bad_maturin_version());
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
        assert!(!without_constraint.warn_bad_maturin_version());
    }

    #[test]
    fn test_warn_incorrect_maturin_version() {
        let without_constraint_dir = TempDir::new().unwrap();
        let pyproject_file = without_constraint_dir.path().join("pyproject.toml");

        fs::write(
            &pyproject_file,
            r#"[build-system]
            requires = ["maturin==0.0.1"]
            build-backend = "maturin"

            [tool.maturin]
            manylinux = "2010"
            "#,
        )
        .unwrap();
        let without_constraint = PyProjectToml::new(pyproject_file).unwrap();
        assert!(!without_constraint.warn_bad_maturin_version());
    }

    #[test]
    fn test_warn_invalid_version_info_conflict() {
        let conflict = toml::from_str::<PyProjectToml>(
            r#"[build-system]
            requires = ["maturin==1.0.0"]

            [project]
            name = "..."
            version = "1.2.3"
            dynamic = ['version']
            "#,
        )
        .unwrap();
        assert!(!conflict.warn_invalid_version_info());
    }

    #[test]
    fn test_warn_invalid_version_info_missing() {
        let missing = toml::from_str::<PyProjectToml>(
            r#"[build-system]
            requires = ["maturin==1.0.0"]

            [project]
            name = "..."
            "#,
        )
        .unwrap();
        assert!(!missing.warn_invalid_version_info());
    }

    #[test]
    fn test_warn_invalid_version_info_ok() {
        let static_ver = toml::from_str::<PyProjectToml>(
            r#"[build-system]
            requires = ["maturin==1.0.0"]

            [project]
            name = "..."
            version = "1.2.3"
            "#,
        )
        .unwrap();
        assert!(static_ver.warn_invalid_version_info());
        let dynamic_ver = toml::from_str::<PyProjectToml>(
            r#"[build-system]
            requires = ["maturin==1.0.0"]

            [project]
            name = "..."
            dynamic = ['version']
            "#,
        )
        .unwrap();
        assert!(dynamic_ver.warn_invalid_version_info());
    }

    #[test]
    fn deserialize_include_exclude() {
        let single = r#"include = ["single"]"#;
        assert_eq!(
            toml::from_str::<ToolMaturin>(single).unwrap().include,
            Some(vec![GlobPattern::Path("single".to_string())])
        );

        let multiple = r#"include = ["one", "two"]"#;
        assert_eq!(
            toml::from_str::<ToolMaturin>(multiple).unwrap().include,
            Some(vec![
                GlobPattern::Path("one".to_string()),
                GlobPattern::Path("two".to_string())
            ])
        );

        let single_format = r#"include = [{path = "path", format="sdist"}]"#;
        assert_eq!(
            toml::from_str::<ToolMaturin>(single_format)
                .unwrap()
                .include,
            Some(vec![GlobPattern::WithFormat {
                path: "path".to_string(),
                format: Formats::Single(Format::Sdist)
            },])
        );

        let multiple_formats = r#"include = [{path = "path", format=["sdist", "wheel"]}]"#;
        assert_eq!(
            toml::from_str::<ToolMaturin>(multiple_formats)
                .unwrap()
                .include,
            Some(vec![GlobPattern::WithFormat {
                path: "path".to_string(),
                format: Formats::Multiple(vec![Format::Sdist, Format::Wheel])
            },])
        );

        let mixed = r#"include = ["one", {path = "two", format="sdist"}, {path = "three", format=["sdist", "wheel"]}]"#;
        assert_eq!(
            toml::from_str::<ToolMaturin>(mixed).unwrap().include,
            Some(vec![
                GlobPattern::Path("one".to_string()),
                GlobPattern::WithFormat {
                    path: "two".to_string(),
                    format: Formats::Single(Format::Sdist),
                },
                GlobPattern::WithFormat {
                    path: "three".to_string(),
                    format: Formats::Multiple(vec![Format::Sdist, Format::Wheel])
                }
            ])
        );
    }

    #[test]
    fn test_gh_1615() {
        let source = indoc!(
            r#"[build-system]
            requires = [ "maturin>=0.14", "numpy", "wheel", "patchelf",]
            build-backend = "maturin"

            [project]
            name = "..."
            license-files = [ "license.txt",]
            requires-python = ">=3.8"
            requires-dist = [ "maturin>=0.14", "...",]
            dependencies = [ "packaging", "...",]
            zip-safe = false
            version = "..."
            readme = "..."
            description = "..."
            classifiers = [ "...",]
        "#
        );
        let temp_dir = TempDir::new().unwrap();
        let pyproject_toml = temp_dir.path().join("pyproject.toml");
        fs::write(&pyproject_toml, source).unwrap();
        let outer_error = PyProjectToml::new(&pyproject_toml).unwrap_err();
        let inner_error = outer_error.source().unwrap();

        let expected = expect![[r#"
            TOML parse error at line 10, column 16
               |
            10 | dependencies = [ "packaging", "...",]
               |                ^^^^^^^^^^^^^^^^^^^^^^
            URL requirement must be preceded by a package name. Add the name of the package before the URL (e.g., `package_name @ /path/to/file`).
            ...
            ^^^
        "#]];
        expected.assert_eq(&inner_error.to_string());
    }
}
