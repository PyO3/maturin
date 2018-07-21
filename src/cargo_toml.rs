use std::collections::HashMap;

/// The `[lib]` section of a Cargo.toml
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CargoTomlLib {
    pub(crate) crate_type: Vec<String>,
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
    pub(crate) metadata: Option<CargoTomlMetadata>,
}

/// Extract of the Cargo.toml that can be reused for the python metadata
///
/// See https://doc.rust-lang.org/cargo/reference/manifest.html for a specification
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct CargoToml {
    pub(crate) lib: CargoTomlLib,
    pub(crate) package: CargoTomlPackage,
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub(crate) struct CargoTomlMetadata {
    #[serde(rename = "pyo3-pack")]
    pub(crate) pyo3_pack: Option<CargoTomlMetadataPyo3Pack>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub(crate) struct CargoTomlMetadataPyo3Pack {
    pub(crate) scripts: Option<HashMap<String, String>>,
}
