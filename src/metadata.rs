use crate::PyProjectToml;
use anyhow::{bail, format_err, Context, Result};
use fs_err as fs;
use indexmap::IndexMap;
use pep440_rs::{Version, VersionSpecifiers};
use pep508_rs::{MarkerExpression, MarkerOperator, MarkerTree, MarkerValue, Requirement};
use pyproject_toml::License;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::str;
use std::str::FromStr;

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
    pub version: Version,
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
    // https://peps.python.org/pep-0639/#license-file-multiple-use
    pub license_files: Vec<PathBuf>,
    pub classifiers: Vec<String>,
    pub requires_dist: Vec<Requirement>,
    pub provides_dist: Vec<String>,
    pub obsoletes_dist: Vec<String>,
    pub requires_python: Option<VersionSpecifiers>,
    pub requires_external: Vec<String>,
    pub project_url: IndexMap<String, String>,
    pub provides_extra: Vec<String>,
    pub scripts: IndexMap<String, String>,
    pub gui_scripts: IndexMap<String, String>,
    pub entry_points: IndexMap<String, IndexMap<String, String>>,
}

impl Metadata21 {
    /// Initializes with name, version and otherwise the defaults
    pub fn new(name: String, version: Version) -> Self {
        Self {
            metadata_version: "2.1".to_string(),
            name,
            version,
            platform: vec![],
            supported_platform: vec![],
            summary: None,
            description: None,
            description_content_type: None,
            keywords: None,
            home_page: None,
            download_url: None,
            author: None,
            author_email: None,
            maintainer: None,
            maintainer_email: None,
            license: None,
            license_files: vec![],
            classifiers: vec![],
            requires_dist: vec![],
            provides_dist: vec![],
            obsoletes_dist: vec![],
            requires_python: None,
            requires_external: vec![],
            project_url: Default::default(),
            provides_extra: vec![],
            scripts: Default::default(),
            gui_scripts: Default::default(),
            entry_points: Default::default(),
        }
    }
}

const PLAINTEXT_CONTENT_TYPE: &str = "text/plain; charset=UTF-8";
const GFM_CONTENT_TYPE: &str = "text/markdown; charset=UTF-8; variant=GFM";

/// Guess a Description-Content-Type based on the file extension,
/// defaulting to plaintext if extension is unknown or empty.
///
/// See https://packaging.python.org/specifications/core-metadata/#description-content-type
fn path_to_content_type(path: &Path) -> String {
    path.extension()
        .map_or(String::from(PLAINTEXT_CONTENT_TYPE), |ext| {
            let ext = ext.to_string_lossy().to_lowercase();
            let type_str = match ext.as_str() {
                "rst" => "text/x-rst; charset=UTF-8",
                "md" => GFM_CONTENT_TYPE,
                "markdown" => GFM_CONTENT_TYPE,
                _ => PLAINTEXT_CONTENT_TYPE,
            };
            String::from(type_str)
        })
}

impl Metadata21 {
    /// Merge metadata with pyproject.toml, where pyproject.toml takes precedence
    ///
    /// pyproject_dir must be the directory containing pyproject.toml
    pub fn merge_pyproject_toml(
        &mut self,
        pyproject_dir: impl AsRef<Path>,
        pyproject_toml: &PyProjectToml,
    ) -> Result<()> {
        let pyproject_dir = pyproject_dir.as_ref();
        if let Some(project) = &pyproject_toml.project {
            self.name = project.name.clone();

            if let Some(version) = &project.version {
                self.version = version.clone();
            }

            if let Some(description) = &project.description {
                self.summary = Some(description.clone());
            }

            match &project.readme {
                Some(pyproject_toml::ReadMe::RelativePath(readme_path)) => {
                    let readme_path = pyproject_dir.join(readme_path);
                    let description = Some(fs::read_to_string(&readme_path).context(format!(
                        "Failed to read readme specified in pyproject.toml, which should be at {}",
                        readme_path.display()
                    ))?);
                    self.description = description;
                    self.description_content_type = Some(path_to_content_type(&readme_path));
                }
                Some(pyproject_toml::ReadMe::Table {
                    file,
                    text,
                    content_type,
                }) => {
                    if file.is_some() && text.is_some() {
                        bail!("file and text fields of 'project.readme' are mutually-exclusive, only one of them should be specified");
                    }
                    if let Some(readme_path) = file {
                        let readme_path = pyproject_dir.join(readme_path);
                        let description = Some(fs::read_to_string(&readme_path).context(format!(
                                "Failed to read readme specified in pyproject.toml, which should be at {}",
                                readme_path.display()
                            ))?);
                        self.description = description;
                    }
                    if let Some(description) = text {
                        self.description = Some(description.clone());
                    }
                    self.description_content_type = content_type.clone();
                }
                None => {}
            }

            if let Some(requires_python) = &project.requires_python {
                self.requires_python = Some(requires_python.clone());
            }

            if let Some(license) = &project.license {
                match license {
                    // TODO: switch to License-Expression core metadata, see https://peps.python.org/pep-0639/#add-license-expression-field
                    License::String(license_expr) => self.license = Some(license_expr.clone()),
                    License::Table { file, text } => match (file, text) {
                        (Some(_), Some(_)) => {
                            bail!("file and text fields of 'project.license' are mutually-exclusive, only one of them should be specified");
                        }
                        (Some(license_path), None) => {
                            let license_path = pyproject_dir.join(license_path);
                            self.license_files.push(license_path);
                        }
                        (None, Some(license_text)) => self.license = Some(license_text.clone()),
                        (None, None) => {}
                    },
                }
            }

            // Until PEP 639 is approved with metadata 2.3, we can assume a
            // dynamic license-files (also awaiting full 2.2 metadata support)
            // We're already emitting the License-Files metadata without issue.
            // license-files.globs = ["LICEN[CS]E*", "COPYING*", "NOTICE*", "AUTHORS*"]
            let license_include_targets = ["LICEN[CS]E*", "COPYING*", "NOTICE*", "AUTHORS*"];
            let escaped_manifest_string = glob::Pattern::escape(pyproject_dir.to_str().unwrap());
            let escaped_manifest_path = Path::new(&escaped_manifest_string);
            for pattern in license_include_targets.iter() {
                for license_path in
                    glob::glob(&escaped_manifest_path.join(pattern).to_string_lossy())?
                        .filter_map(Result::ok)
                {
                    // if the pyproject.toml specified the license file,
                    // then we won't list it as automatically included
                    if !self.license_files.contains(&license_path) {
                        eprintln!("📦 Including license file \"{}\"", license_path.display());
                        self.license_files.push(license_path);
                    }
                }
            }

            if let Some(authors) = &project.authors {
                let mut names = Vec::with_capacity(authors.len());
                let mut emails = Vec::with_capacity(authors.len());
                for author in authors {
                    match (&author.name, &author.email) {
                        (Some(name), Some(email)) => {
                            emails.push(format!("{name} <{email}>"));
                        }
                        (Some(name), None) => {
                            names.push(name.as_str());
                        }
                        (None, Some(email)) => {
                            emails.push(email.clone());
                        }
                        (None, None) => {}
                    }
                }
                if !names.is_empty() {
                    self.author = Some(names.join(", "));
                }
                if !emails.is_empty() {
                    self.author_email = Some(emails.join(", "));
                }
            }

            if let Some(maintainers) = &project.maintainers {
                let mut names = Vec::with_capacity(maintainers.len());
                let mut emails = Vec::with_capacity(maintainers.len());
                for maintainer in maintainers {
                    match (&maintainer.name, &maintainer.email) {
                        (Some(name), Some(email)) => {
                            emails.push(format!("{name} <{email}>"));
                        }
                        (Some(name), None) => {
                            names.push(name.as_str());
                        }
                        (None, Some(email)) => {
                            emails.push(email.clone());
                        }
                        (None, None) => {}
                    }
                }
                if !names.is_empty() {
                    self.maintainer = Some(names.join(", "));
                }
                if !emails.is_empty() {
                    self.maintainer_email = Some(emails.join(", "));
                }
            }

            if let Some(keywords) = &project.keywords {
                self.keywords = Some(keywords.join(","));
            }

            if let Some(classifiers) = &project.classifiers {
                self.classifiers = classifiers.clone();
            }

            if let Some(urls) = &project.urls {
                self.project_url = urls.clone();
            }

            if let Some(dependencies) = &project.dependencies {
                self.requires_dist = dependencies.clone();
            }

            if let Some(dependencies) = &project.optional_dependencies {
                // Transform the extra -> deps map into the PEP 508 style `dep ; extras = ...` style
                for (extra, deps) in dependencies {
                    self.provides_extra.push(extra.clone());
                    for dep in deps {
                        let mut dep = dep.clone();
                        // Keep in sync with `develop()`!
                        let new_extra = MarkerTree::Expression(MarkerExpression {
                            l_value: MarkerValue::Extra,
                            operator: MarkerOperator::Equal,
                            r_value: MarkerValue::QuotedString(extra.to_string()),
                        });
                        if let Some(existing) = dep.marker.take() {
                            dep.marker = Some(MarkerTree::And(vec![existing, new_extra]));
                        } else {
                            dep.marker = Some(new_extra);
                        }
                        self.requires_dist.push(dep);
                    }
                }
            }

            if let Some(scripts) = &project.scripts {
                self.scripts = scripts.clone();
            }
            if let Some(gui_scripts) = &project.gui_scripts {
                self.gui_scripts = gui_scripts.clone();
            }
            if let Some(entry_points) = &project.entry_points {
                // Raise error on ambiguous entry points: https://www.python.org/dev/peps/pep-0621/#entry-points
                if entry_points.contains_key("console_scripts") {
                    bail!("console_scripts is not allowed in project.entry-points table");
                }
                if entry_points.contains_key("gui_scripts") {
                    bail!("gui_scripts is not allowed in project.entry-points table");
                }
                self.entry_points = entry_points.clone();
            }
        }
        Ok(())
    }

    /// Uses a Cargo.toml to create the metadata for python packages
    ///
    /// manifest_path must be the directory, not the file
    pub fn from_cargo_toml(
        manifest_path: impl AsRef<Path>,
        cargo_metadata: &cargo_metadata::Metadata,
    ) -> Result<Metadata21> {
        let package = cargo_metadata
            .root_package()
            .context("Expected cargo to return metadata with root_package")?;
        let authors = package.authors.join(", ");
        let author_email = if authors.contains('@') {
            Some(authors.clone())
        } else {
            None
        };

        let mut description: Option<String> = None;
        let mut description_content_type: Option<String> = None;
        // See https://packaging.python.org/specifications/core-metadata/#description
        // and https://doc.rust-lang.org/cargo/reference/manifest.html#the-readme-field
        if package.readme == Some("false".into()) {
            // > You can suppress this behavior by setting this field to false
        } else if let Some(ref readme) = package.readme {
            let readme_path = manifest_path.as_ref().join(readme);
            description = Some(fs::read_to_string(&readme_path).context(format!(
                "Failed to read Readme specified in Cargo.toml, which should be at {}",
                readme_path.display()
            ))?);

            description_content_type = Some(path_to_content_type(&readme_path));
        } else {
            // > If no value is specified for this field, and a file named
            // > README.md, README.txt or README exists in the package root
            // Even though it's not what cargo does, we also search for README.rst
            // since it's still popular in the python world
            for readme_guess in ["README.md", "README.txt", "README.rst", "README"] {
                let guessed_readme = manifest_path.as_ref().join(readme_guess);
                if guessed_readme.exists() {
                    let context = format!(
                        "Readme at {} exists, but can't be read",
                        guessed_readme.display()
                    );
                    description = Some(fs::read_to_string(&guessed_readme).context(context)?);
                    description_content_type = Some(path_to_content_type(&guessed_readme));
                    break;
                }
            }
        };
        let name = package.name.clone();
        let mut project_url = IndexMap::new();
        if let Some(repository) = package.repository.as_ref() {
            project_url.insert("Source Code".to_string(), repository.clone());
        }
        let license_files = if let Some(license_file) = package.license_file.as_ref() {
            vec![manifest_path.as_ref().join(license_file)]
        } else {
            Vec::new()
        };

        let version = Version::from_str(&package.version.to_string()).map_err(|err| {
            format_err!(
                "Rust version used in Cargo.toml is not a valid python version: {}. \
                    Note that rust uses [SemVer](https://semver.org/) while python uses \
                    [PEP 440](https://peps.python.org/pep-0440/), which have e.g. some differences \
                    when declaring prereleases.",
                err
            )
        })?;
        let metadata = Metadata21 {
            // name, version and metadata_version are added through Metadata21::new()
            // Mapped from cargo metadata
            summary: package.description.clone(),
            description,
            description_content_type,
            keywords: if package.keywords.is_empty() {
                None
            } else {
                Some(package.keywords.join(","))
            },
            home_page: package.homepage.clone(),
            download_url: None,
            // Cargo.toml has no distinction between author and author email
            author: if package.authors.is_empty() {
                None
            } else {
                Some(authors)
            },
            author_email,
            license: package.license.clone(),
            license_files,
            project_url,
            ..Metadata21::new(name, version)
        };
        Ok(metadata)
    }

    /// Formats the metadata into a list where keys with multiple values
    /// become multiple single-valued key-value pairs. This format is needed for the pypi
    /// uploader and for the METADATA file inside wheels
    pub fn to_vec(&self) -> Vec<(String, String)> {
        let mut fields = vec![
            ("Metadata-Version", self.metadata_version.clone()),
            ("Name", self.name.clone()),
            ("Version", self.version.to_string()),
        ];

        let mut add_vec = |name, values: &[String]| {
            for i in values {
                fields.push((name, i.clone()));
            }
        };

        add_vec("Platform", &self.platform);
        add_vec("Supported-Platform", &self.supported_platform);
        add_vec("Classifier", &self.classifiers);
        add_vec(
            "Requires-Dist",
            &self
                .requires_dist
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
        );
        add_vec("Provides-Dist", &self.provides_dist);
        add_vec("Obsoletes-Dist", &self.obsoletes_dist);
        add_vec("Requires-External", &self.requires_external);
        add_vec("Provides-Extra", &self.provides_extra);

        let license_files: Vec<String> = self
            .license_files
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        add_vec("License-File", &license_files);

        let mut add_option = |name, value: &Option<String>| {
            if let Some(some) = value.clone() {
                fields.push((name, some));
            }
        };

        add_option("Summary", &self.summary);
        add_option("Keywords", &self.keywords);
        add_option("Home-Page", &self.home_page);
        add_option("Download-URL", &self.download_url);
        add_option("Author", &self.author);
        add_option("Author-email", &self.author_email);
        add_option("Maintainer", &self.maintainer);
        add_option("Maintainer-email", &self.maintainer_email);
        add_option("License", &self.license.as_deref().map(fold_header));
        add_option(
            "Requires-Python",
            &self
                .requires_python
                .as_ref()
                .map(|requires_python| requires_python.to_string()),
        );
        add_option("Description-Content-Type", &self.description_content_type);
        // Project-URL is special
        // "A string containing a browsable URL for the project and a label for it, separated by a comma."
        // `Project-URL: Bug Tracker, http://bitbucket.org/tarek/distribute/issues/`
        for (key, value) in self.project_url.iter() {
            fields.push(("Project-URL", format!("{key}, {value}")))
        }

        // Description shall be last, so we can ignore RFC822 and just put the description
        // in the body
        // The borrow checker doesn't like us using add_option here
        if let Some(description) = &self.description {
            fields.push(("Description", description.clone()));
        }

        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    /// Writes the format for the metadata file inside wheels
    pub fn to_file_contents(&self) -> Result<String> {
        let mut fields = self.to_vec();
        let mut out = "".to_string();
        let body = match fields.last() {
            Some((key, description)) if key == "Description" => {
                let desc = description.clone();
                fields.pop().unwrap();
                Some(desc)
            }
            Some((_, _)) => None,
            None => None,
        };

        for (key, value) in fields {
            writeln!(out, "{key}: {value}")?;
        }

        if let Some(body) = body {
            writeln!(out, "\n{body}")?;
        }

        Ok(out)
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
        self.version.to_string().replace('-', "_")
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

/// Fold long header field according to RFC 5322 section 2.2.3
/// https://datatracker.ietf.org/doc/html/rfc5322#section-2.2.3
fn fold_header(text: &str) -> String {
    let mut result = String::with_capacity(text.len());

    let options = textwrap::Options::new(78)
        .initial_indent("")
        .subsequent_indent("\t");
    for (i, line) in textwrap::wrap(text, options).iter().enumerate() {
        if i > 0 {
            result.push_str("\r\n");
        }
        let line = line.trim_end();
        if line.is_empty() {
            result.push('\t');
        } else {
            result.push_str(line);
        }
    }

    result
}

#[cfg(test)]
mod test {
    use super::*;
    use cargo_metadata::MetadataCommand;
    use expect_test::{expect, Expect};
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    fn assert_metadata_from_cargo_toml(
        readme: &str,
        cargo_toml: &str,
        expected: Expect,
    ) -> Metadata21 {
        let crate_dir = tempfile::tempdir().unwrap();
        let crate_path = crate_dir.path();
        let manifest_path = crate_path.join("Cargo.toml");
        fs::create_dir(crate_path.join("src")).unwrap();
        fs::write(crate_path.join("src/lib.rs"), "").unwrap();

        let readme_path = crate_path.join("README.md");
        fs::write(&readme_path, readme.as_bytes()).unwrap();

        let readme_path = if cfg!(windows) {
            readme_path.to_str().unwrap().replace('\\', "/")
        } else {
            readme_path.to_str().unwrap().to_string()
        };

        let toml_with_path = cargo_toml.replace("REPLACE_README_PATH", &readme_path);
        fs::write(&manifest_path, toml_with_path).unwrap();

        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_path)
            .exec()
            .unwrap();

        let metadata = Metadata21::from_cargo_toml(crate_path, &cargo_metadata).unwrap();

        let actual = metadata.to_file_contents().unwrap();

        expected.assert_eq(&actual);

        // get_dist_info_dir test checks against hard-coded values - check that they are as expected in the source first
        assert!(
            cargo_toml.contains("name = \"info-project\"")
                && cargo_toml.contains("version = \"0.1.0\""),
            "cargo_toml name and version string do not match hardcoded values, test will fail",
        );

        metadata
    }

    #[test]
    fn test_metadata_from_cargo_toml() {
        let readme = indoc!(
            r#"
            # Some test package

            This is the readme for a test package
        "#
        );

        let cargo_toml = indoc!(
            r#"
            [package]
            authors = ["konstin <konstin@mailbox.org>"]
            name = "info-project"
            version = "0.1.0"
            description = "A test project"
            homepage = "https://example.org"
            readme = "REPLACE_README_PATH"
            keywords = ["ffi", "test"]

            [lib]
            crate-type = ["cdylib"]
            name = "pyo3_pure"
        "#
        );

        let expected = expect![[r#"
            Metadata-Version: 2.1
            Name: info-project
            Version: 0.1.0
            Summary: A test project
            Keywords: ffi,test
            Home-Page: https://example.org
            Author: konstin <konstin@mailbox.org>
            Author-email: konstin <konstin@mailbox.org>
            Description-Content-Type: text/markdown; charset=UTF-8; variant=GFM

            # Some test package

            This is the readme for a test package

        "#]];

        assert_metadata_from_cargo_toml(readme, cargo_toml, expected);
    }

    #[test]
    fn test_path_to_content_type() {
        for (filename, expected) in &[
            ("r.md", GFM_CONTENT_TYPE),
            ("r.markdown", GFM_CONTENT_TYPE),
            ("r.mArKdOwN", GFM_CONTENT_TYPE),
            ("r.rst", "text/x-rst; charset=UTF-8"),
            ("r.somethingelse", PLAINTEXT_CONTENT_TYPE),
            ("r", PLAINTEXT_CONTENT_TYPE),
        ] {
            let result = path_to_content_type(&PathBuf::from(filename));
            assert_eq!(
                &result.as_str(),
                expected,
                "Wrong content type for file '{}'. Expected '{}', got '{}'",
                filename,
                expected,
                result
            );
        }
    }

    #[test]
    fn test_merge_metadata_from_pyproject_toml() {
        let manifest_dir = PathBuf::from("test-crates").join("pyo3-pure");
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_dir.join("Cargo.toml"))
            .exec()
            .unwrap();
        let mut metadata = Metadata21::from_cargo_toml(&manifest_dir, &cargo_metadata).unwrap();
        let pyproject_toml = PyProjectToml::new(manifest_dir.join("pyproject.toml")).unwrap();
        metadata
            .merge_pyproject_toml(&manifest_dir, &pyproject_toml)
            .unwrap();
        assert_eq!(
            metadata.summary,
            Some("Implements a dummy function in Rust".to_string())
        );
        assert_eq!(
            metadata.description,
            Some(fs_err::read_to_string("test-crates/pyo3-pure/README.md").unwrap())
        );
        assert_eq!(metadata.classifiers, &["Programming Language :: Rust"]);
        assert_eq!(
            metadata.maintainer_email,
            Some("messense <messense@icloud.com>".to_string())
        );
        assert_eq!(metadata.scripts["get_42"], "pyo3_pure:DummyClass.get_42");
        assert_eq!(
            metadata.gui_scripts["get_42_gui"],
            "pyo3_pure:DummyClass.get_42"
        );
        assert_eq!(metadata.provides_extra, &["test"]);
        assert_eq!(
            metadata.requires_dist,
            &[
                Requirement::from_str("attrs; extra == 'test'",).unwrap(),
                Requirement::from_str("boltons; (sys_platform == 'win32') and extra == 'test'")
                    .unwrap(),
            ]
        );
        assert_eq!(metadata.license.as_ref().unwrap(), "MIT");

        let license_file = &metadata.license_files[0];
        assert_eq!(license_file.file_name().unwrap(), "LICENSE");

        let content = metadata.to_file_contents().unwrap();
        let pkginfo: Result<python_pkginfo::Metadata, _> = content.parse();
        assert!(pkginfo.is_ok());
    }

    #[test]
    fn test_merge_metadata_from_pyproject_toml_with_customized_python_source_dir() {
        let manifest_dir = PathBuf::from("test-crates").join("pyo3-mixed-py-subdir");
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_dir.join("Cargo.toml"))
            .exec()
            .unwrap();
        let mut metadata = Metadata21::from_cargo_toml(&manifest_dir, &cargo_metadata).unwrap();
        let pyproject_toml = PyProjectToml::new(manifest_dir.join("pyproject.toml")).unwrap();
        metadata
            .merge_pyproject_toml(&manifest_dir, &pyproject_toml)
            .unwrap();
        // defined in Cargo.toml
        assert_eq!(
            metadata.summary,
            Some("Implements a dummy function combining rust and python".to_string())
        );
        // defined in pyproject.toml
        assert_eq!(metadata.scripts["get_42"], "pyo3_mixed_py_subdir:get_42");
    }

    #[test]
    fn test_implicit_readme() {
        let manifest_dir = PathBuf::from("test-crates").join("pyo3-mixed");
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_dir.join("Cargo.toml"))
            .exec()
            .unwrap();
        let metadata = Metadata21::from_cargo_toml(&manifest_dir, &cargo_metadata).unwrap();
        assert!(metadata.description.unwrap().starts_with("# pyo3-mixed"));
        assert_eq!(
            metadata.description_content_type.unwrap(),
            "text/markdown; charset=UTF-8; variant=GFM"
        );
    }

    #[test]
    fn test_merge_metadata_from_pyproject_dynamic_license_test() {
        let manifest_dir = PathBuf::from("test-crates").join("license-test");
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_dir.join("Cargo.toml"))
            .exec()
            .unwrap();
        let mut metadata = Metadata21::from_cargo_toml(&manifest_dir, &cargo_metadata).unwrap();
        let pyproject_toml = PyProjectToml::new(manifest_dir.join("pyproject.toml")).unwrap();
        metadata
            .merge_pyproject_toml(&manifest_dir, &pyproject_toml)
            .unwrap();

        // verify Cargo.toml value came through
        assert_eq!(metadata.license.as_ref().unwrap(), "MIT");

        // verify we have the total number of expected licenses
        assert_eq!(4, metadata.license_files.len());

        // Verify pyproject.toml license = {file = ...} worked
        assert_eq!(metadata.license_files[0], manifest_dir.join("LICENCE.txt"));

        // Verify the default licenses were included
        assert_eq!(metadata.license_files[1], manifest_dir.join("LICENSE"));
        assert_eq!(metadata.license_files[2], manifest_dir.join("NOTICE.md"));
        assert_eq!(metadata.license_files[3], manifest_dir.join("AUTHORS.txt"));
    }
}
