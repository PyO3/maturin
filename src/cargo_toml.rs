use failure::{Error, ResultExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
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
    // Those three fields are mandatory
    // https://doc.rust-lang.org/cargo/reference/manifest.html#the-package-section
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) authors: Vec<String>,
    // All other fields are optional
    pub(crate) description: Option<String>,
    pub(crate) documentation: Option<String>,
    pub(crate) homepage: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) readme: Option<String>,
    pub(crate) keywords: Option<Vec<String>>,
    pub(crate) categories: Option<Vec<String>>,
    pub(crate) license: Option<String>,
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
    pub fn from_path(manifest_file: impl AsRef<Path>) -> Result<Self, Error> {
        let contents = fs::read_to_string(&manifest_file).context(format!(
            "Can't read Cargo.toml at {}",
            manifest_file.as_ref().display(),
        ))?;
        let cargo_toml = toml::from_str(&contents).context(format!(
            "Failed to parse Cargo.toml at {}",
            manifest_file.as_ref().display()
        ))?;
        Ok(cargo_toml)
    }

    /// Returns the python entrypoints
    pub fn scripts(&self) -> HashMap<String, String> {
        match self.package.metadata {
            Some(CargoTomlMetadata {
                pyo3_pack:
                    Some(CargoTomlMetadataPyo3Pack {
                        scripts: Some(ref scripts),
                        ..
                    }),
            }) => scripts.clone(),
            _ => HashMap::new(),
        }
    }

    /// Returns the trove classifier
    pub fn classifier(&self) -> Vec<String> {
        match self.package.metadata {
            Some(CargoTomlMetadata {
                pyo3_pack:
                    Some(CargoTomlMetadataPyo3Pack {
                        classifier: Some(ref classifier),
                        ..
                    }),
            }) => classifier.clone(),
            _ => Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
struct CargoTomlMetadata {
    #[serde(rename = "pyo3-pack")]
    pyo3_pack: Option<CargoTomlMetadataPyo3Pack>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
struct CargoTomlMetadataPyo3Pack {
    scripts: Option<HashMap<String, String>>,
    classifier: Option<Vec<String>>,
}

#[cfg(test)]
mod test {
    use super::*;
    use indoc::indoc;
    use toml;

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

            [package.metadata.pyo3-pack.scripts]
            ph = "pyo3_pack:print_hello"

            [package.metadata.pyo3-pack]
            classifier = ["Programming Language :: Python"]
        "#
        );

        let cargo_toml: CargoToml = toml::from_str(&cargo_toml).unwrap();

        let mut scripts = HashMap::new();
        scripts.insert("ph".to_string(), "pyo3_pack:print_hello".to_string());

        let classifier = vec!["Programming Language :: Python".to_string()];

        assert_eq!(cargo_toml.scripts(), scripts);
        assert_eq!(cargo_toml.classifier(), classifier);
    }
}
