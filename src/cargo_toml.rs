use anyhow::{Context, Result};
use fs_err as fs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The `[lib]` section of a Cargo.toml
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CargoTomlLib {
    pub(crate) crate_type: Option<Vec<String>>,
    pub(crate) name: Option<String>,
}

/// The `[package]` section of a Cargo.toml
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CargoTomlPackage {
    pub(crate) name: String,
    metadata: Option<CargoTomlMetadata>,
}

/// Extract of the Cargo.toml that can be reused for the python metadata
///
/// See https://doc.rust-lang.org/cargo/reference/manifest.html for a specification
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct CargoToml {
    pub(crate) lib: Option<CargoTomlLib>,
    pub(crate) package: CargoTomlPackage,
}

impl CargoToml {
    /// Reads and parses the Cargo.toml at the given location
    pub fn from_path(manifest_file: impl AsRef<Path>) -> Result<Self> {
        let contents = fs::read_to_string(&manifest_file).context(format!(
            "Can't read Cargo.toml at {}",
            manifest_file.as_ref().display(),
        ))?;
        let cargo_toml = toml_edit::easy::from_str(&contents).context(format!(
            "Failed to parse Cargo.toml at {}",
            manifest_file.as_ref().display()
        ))?;
        Ok(cargo_toml)
    }

    /// Returns the python entrypoints
    pub fn scripts(&self) -> HashMap<String, String> {
        match self.package.metadata {
            Some(CargoTomlMetadata {
                maturin:
                    Some(RemainingCoreMetadata {
                        scripts: Some(ref scripts),
                        ..
                    }),
            }) => scripts.clone(),
            _ => HashMap::new(),
        }
    }

    /// Returns the trove classifiers
    pub fn classifiers(&self) -> Vec<String> {
        match self.package.metadata {
            Some(CargoTomlMetadata {
                maturin:
                    Some(RemainingCoreMetadata {
                        classifiers: Some(ref classifier),
                        ..
                    }),
            }) => classifier.clone(),
            _ => Vec::new(),
        }
    }

    /// Returns the value of `[project.metadata.maturin]` or an empty stub
    pub fn remaining_core_metadata(&self) -> RemainingCoreMetadata {
        match &self.package.metadata {
            Some(CargoTomlMetadata {
                maturin: Some(extra_metadata),
            }) => extra_metadata.clone(),
            _ => Default::default(),
        }
    }

    /// Warn about deprecated python metadata support
    pub fn warn_deprecated_python_metadata(&self) -> bool {
        let mut deprecated = Vec::new();
        if let Some(CargoTomlMetadata {
            maturin: Some(extra_metadata),
        }) = &self.package.metadata
        {
            if extra_metadata.scripts.is_some() {
                deprecated.push("scripts");
            }
            if extra_metadata.classifiers.is_some() {
                deprecated.push("classifiers");
            }
            if extra_metadata.maintainer.is_some() {
                deprecated.push("maintainer");
            }
            if extra_metadata.maintainer_email.is_some() {
                deprecated.push("maintainer-email");
            }
            if extra_metadata.requires_dist.is_some() {
                deprecated.push("requires-dist");
            }
            if extra_metadata.requires_python.is_some() {
                deprecated.push("requires-python");
            }
            if extra_metadata.requires_external.is_some() {
                deprecated.push("requires-external");
            }
            if extra_metadata.project_url.is_some() {
                deprecated.push("project-url");
            }
            if extra_metadata.provides_extra.is_some() {
                deprecated.push("provides-extra");
            }
            if extra_metadata.description_content_type.is_some() {
                deprecated.push("description-content-type");
            }
        }
        if !deprecated.is_empty() {
            println!(
                "⚠️  Warning: the following metadata fields in `package.metadata.maturin` section \
                of Cargo.toml are deprecated and will be removed in future versions: {}, \
                please set them in pyproject.toml as PEP 621 specifies.",
                deprecated.join(", ")
            );
            true
        } else {
            false
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
struct CargoTomlMetadata {
    maturin: Option<RemainingCoreMetadata>,
}

/// The `[project.metadata.maturin]` with the python specific metadata
///
/// Those fields are the part of the
/// [python core metadata](https://packaging.python.org/specifications/core-metadata/)
/// that doesn't have an equivalent in cargo's `[package]` table
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct RemainingCoreMetadata {
    pub name: Option<String>,
    pub scripts: Option<HashMap<String, String>>,
    // For backward compatibility, we also allow classifier.
    #[serde(alias = "classifier")]
    pub classifiers: Option<Vec<String>>,
    pub maintainer: Option<String>,
    pub maintainer_email: Option<String>,
    pub requires_dist: Option<Vec<String>>,
    pub requires_python: Option<String>,
    pub requires_external: Option<Vec<String>>,
    pub project_url: Option<HashMap<String, String>>,
    pub provides_extra: Option<Vec<String>>,
    pub description_content_type: Option<String>,
    /// The directory with python module, contains `<module_name>/__init__.py`
    pub python_source: Option<String>,
    /// The directory containing the wheel data
    pub data: Option<String>,
}

#[cfg(test)]
mod test {
    use super::*;
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_metadata_from_cargo_toml() {
        let cargo_toml = indoc!(
            r#"
            [package]
            authors = ["konstin <konstin@mailbox.org>"]
            name = "info-project"
            version = "0.1.0"
            description = "A test project"
            homepage = "https://example.org"
            keywords = ["ffi", "test"]

            [lib]
            crate-type = ["cdylib"]
            name = "pyo3_pure"

            [package.metadata.maturin.scripts]
            ph = "maturin:print_hello"

            [package.metadata.maturin]
            classifiers = ["Programming Language :: Python"]
            requires-dist = ["flask~=1.1.0", "toml==0.10.0"]
        "#
        );

        let cargo_toml: CargoToml = toml_edit::easy::from_str(cargo_toml).unwrap();

        let mut scripts = HashMap::new();
        scripts.insert("ph".to_string(), "maturin:print_hello".to_string());

        let classifiers = vec!["Programming Language :: Python".to_string()];

        let requires_dist = Some(vec!["flask~=1.1.0".to_string(), "toml==0.10.0".to_string()]);

        assert_eq!(cargo_toml.scripts(), scripts);
        assert_eq!(cargo_toml.classifiers(), classifiers);
        assert_eq!(
            cargo_toml.remaining_core_metadata().requires_dist,
            requires_dist
        );
    }

    #[test]
    fn test_old_classifier_works() {
        let cargo_toml = indoc!(
            r#"
            [package]
            authors = ["konstin <konstin@mailbox.org>"]
            name = "info-project"
            version = "0.1.0"
            description = "A test project"
            homepage = "https://example.org"
            keywords = ["ffi", "test"]

            [lib]
            crate-type = ["cdylib"]
            name = "pyo3_pure"

            [package.metadata.maturin]
            # Not classifiers
            classifier = ["Programming Language :: Python"]
        "#
        );

        let cargo_toml: CargoToml = toml_edit::easy::from_str(cargo_toml).unwrap();

        let classifiers = vec!["Programming Language :: Python".to_string()];

        assert_eq!(cargo_toml.classifiers(), classifiers);
        assert!(cargo_toml.warn_deprecated_python_metadata());
    }

    #[test]
    fn test_metadata_from_cargo_toml_without_authors() {
        let cargo_toml = indoc!(
            r#"
            [package]
            name = "info-project"
            version = "0.1.0"
            description = "A test project"
            homepage = "https://example.org"
            keywords = ["ffi", "test"]

            [lib]
            crate-type = ["cdylib"]
            name = "pyo3_pure"

            [package.metadata.maturin.scripts]
            ph = "maturin:print_hello"

            [package.metadata.maturin]
            classifiers = ["Programming Language :: Python"]
            requires-dist = ["flask~=1.1.0", "toml==0.10.0"]
        "#
        );

        let cargo_toml: Result<CargoToml, _> = toml_edit::easy::from_str(cargo_toml);
        assert!(cargo_toml.is_ok());
    }
}
