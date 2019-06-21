use crate::CargoToml;
use failure::{Error, ResultExt};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};
use std::str;

/// The metadata required to generate the .dist-info directory
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct WheelMetadata {
    /// Python Package Metadata 2.1
    pub metadata21: Metadata21,
    /// The `[console_scripts]` for the entry_points.txt
    pub scripts: HashMap<String, String>,
    /// The name of the module can be distinct from the package name, mostly
    /// because package names normally contain minuses while module names
    /// have underscores. The package name is part of metadata21
    pub module_name: String,
}

/// Python Package Metadata 2.1 as specified in
/// https://packaging.python.org/specifications/core-metadata/
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
#[allow(missing_docs)]
pub struct Metadata21 {
    // Mandatory fields
    pub metadata_version: String,
    pub name: String,
    pub version: String,
    // Optional fields
    pub platform: Vec<String>,
    pub supported_platform: Vec<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub description_content_type: Option<String>,
    pub keywords: Option<String>,
    pub home_page: Option<String>,
    pub download_url: Option<String>,
    pub author: Option<String>,
    pub author_email: Option<String>,
    pub maintainer: Option<String>,
    pub maintainer_email: Option<String>,
    pub license: Option<String>,
    pub classifier: Vec<String>,
    pub requires_dist: Vec<String>,
    pub provides_dist: Vec<String>,
    pub obsoletes_dist: Vec<String>,
    pub requires_python: Option<String>,
    pub requires_external: Vec<String>,
    pub project_url: Vec<String>,
    pub provides_extra: Vec<String>,
}

impl Metadata21 {
    /// Uses a Cargo.toml to create the metadata for python packages
    ///
    /// manifest_path must be the directory, not the file
    pub fn from_cargo_toml(
        cargo_toml: &CargoToml,
        manifest_path: impl AsRef<Path>,
    ) -> Result<Metadata21, Error> {
        let authors = cargo_toml.package.authors.join(", ");

        // See https://packaging.python.org/specifications/core-metadata/#description
        let description = if let Some(ref readme) = cargo_toml.package.readme {
            Some(
                read_to_string(manifest_path.as_ref().join(readme)).context(format!(
                    "Failed to read readme specified in Cargo.toml, which should be at {}",
                    manifest_path.as_ref().join(readme).display()
                ))?,
            )
        } else {
            None
        };

        let description_content_type = if description.is_some() {
            // I'm not hundred percent sure if that's the best preset
            Some("text/markdown; charset=UTF-8; variant=GFM".to_owned())
        } else {
            None
        };

        let classifier = cargo_toml.classifier();

        Ok(Metadata21 {
            metadata_version: "2.1".to_owned(),
            name: cargo_toml.package.name.to_owned(),
            version: cargo_toml.package.version.clone(),
            platform: Vec::new(),
            supported_platform: Vec::new(),
            summary: cargo_toml.package.description.clone(),
            description,
            description_content_type,
            keywords: cargo_toml
                .package
                .keywords
                .clone()
                .map(|keywords| keywords.join(" ")),
            home_page: cargo_toml.package.homepage.clone(),
            download_url: None,
            // Cargo.toml has no distinction between author and author email
            author: Some(authors.to_owned()),
            author_email: Some(authors.to_owned()),
            maintainer: None,
            maintainer_email: None,
            license: cargo_toml.package.license.clone(),
            classifier,
            requires_dist: Vec::new(),
            provides_dist: Vec::new(),
            obsoletes_dist: Vec::new(),
            requires_python: None,
            requires_external: Vec::new(),
            project_url: Vec::new(),
            provides_extra: Vec::new(),
        })
    }

    /// Formats the metadata into a list of key with multiple values have
    /// mutliple singel-valued pairs. This format is needed for the pypi
    /// uploader and for the metadata file inside wheels
    pub fn to_vec(&self) -> Vec<(String, String)> {
        let mut fields = vec![
            ("Metadata-Version", self.metadata_version.clone()),
            ("Name", self.name.clone()),
            ("Version", self.version.clone()),
        ];

        macro_rules! vec_types {
            ($(($name:tt : $value:ident)),*) => {
                $(
                    for i in &self.$value {
                        fields.push(($name, i.to_string()));
                    }
                )*
            }
        };

        macro_rules! option_types {
            ($(($name:tt : $value:ident)),*) => {
                $(
                    if let Some(some) = self.$value.clone() {
                        fields.push(($name, some));
                    }
                )*
            }
        }

        vec_types![
            ("Supported-Platform": supported_platform),
            ("Platform": platform),
            ("Supported-Platform": supported_platform),
            ("Classifier": classifier),
            ("Requires-Dist": requires_dist),
            ("Provides-Dist": provides_dist),
            ("Obsoletes-Dist": obsoletes_dist),
            ("Requires-External": requires_external),
            ("Project-Url": project_url),
            ("Provides-Extra": provides_extra)
        ];

        option_types![
            ("Summary": summary),
            ("Keywords": keywords),
            ("Home-Page": home_page),
            ("Download-Url": download_url),
            ("Author": author),
            ("Author-Email": author_email),
            ("Maintainer": maintainer),
            ("Maintainer-Email": maintainer_email),
            ("License": license),
            ("Requires-Python": requires_python),
            ("Description-Content-Type": description_content_type),
            // Description shall be last, so we can ignore RFC822 and just put the description
            // in the body
            ("Description": description)
        ];

        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    /// Writes the format for the metadata file inside wheels
    pub fn to_file_contents(&self) -> String {
        let mut fields = self.to_vec();
        let mut out = "".to_string();

        let body = match fields.clone().last() {
            Some((key, description)) if key == "Description" => {
                fields.pop().unwrap();
                Some(description.clone())
            }
            Some((_, _)) => None,
            None => None,
        };

        for (key, value) in fields {
            out += &format!("{}: {}\n", key, value);
        }

        if let Some(body) = body {
            out += &format!("\n{}\n", body);
        }

        out
    }

    /// Returns the distribution name according to PEP 427, Section "Escaping
    /// and Unicode"
    pub fn get_distribution_escaped(&self) -> String {
        let re = Regex::new(r"[^\w\d.]+").unwrap();
        re.replace_all(&self.name, "_").to_string()
    }

    /// Returns the version encoded according to PEP 427, Section "Escaping
    /// and Unicode"
    pub fn get_version_escaped(&self) -> String {
        let re = Regex::new(r"[^\w\d.]+").unwrap();
        re.replace_all(&self.version, "_").to_string()
    }

    /// Returns the name of the .dist-info directory as defined in the wheel specification
    pub fn get_dist_info_dir(&self) -> PathBuf {
        PathBuf::from(format!(
            "{}-{}.dist-info",
            &self.get_distribution_escaped(),
            &self.get_version_escaped()
        ))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use indoc::indoc;
    use std::io::Write;
    use tempfile;
    use toml;

    #[test]
    fn test_metadata_from_cargo_toml() {
        let readme = indoc!(
            r#"
            # Some test package

            This is the readme for a test package
        "#
        );

        let mut readme_md = tempfile::NamedTempFile::new().unwrap();

        let readme_path = if cfg!(windows) {
            readme_md.path().to_str().unwrap().replace("\\", "/")
        } else {
            readme_md.path().to_str().unwrap().to_string()
        };

        readme_md.write_all(readme.as_bytes()).unwrap();

        let cargo_toml = indoc!(
            r#"
            [package]
            authors = ["konstin <konstin@mailbox.org>"]
            name = "info-project"
            version = "0.1.0"
            description = "A test project"
            homepage = "https://example.org"
            readme = "readme.md"
            keywords = ["ffi", "test"]

            [lib]
            crate-type = ["cdylib"]
            name = "pyo3_pure"

            [package.metadata.pyo3-pack.scripts]
            ph = "pyo3_pack:print_hello"

            [package.metadata.pyo3-pack]
            classifier = ["Programming Language :: Python"]
        "#
        )
        .replace("readme.md", &readme_path);

        let cargo_toml: CargoToml = toml::from_str(&cargo_toml).unwrap();

        let metadata =
            Metadata21::from_cargo_toml(&cargo_toml, &readme_md.path().parent().unwrap()).unwrap();

        let expected = indoc!(
            r#"
            Metadata-Version: 2.1
            Name: info-project
            Version: 0.1.0
            Classifier: Programming Language :: Python
            Summary: A test project
            Keywords: ffi test
            Home-Page: https://example.org
            Author: konstin <konstin@mailbox.org>
            Author-Email: konstin <konstin@mailbox.org>
            Description-Content-Type: text/markdown; charset=UTF-8; variant=GFM

            # Some test package

            This is the readme for a test package
        "#
        );

        let actual = metadata.to_file_contents();

        assert_eq!(actual.trim(), expected.trim());

        assert_eq!(
            metadata.get_dist_info_dir(),
            PathBuf::from("info_project-0.1.0.dist-info")
        )
    }
}
