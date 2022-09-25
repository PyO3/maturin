use crate::{CargoToml, PyProjectToml};
use anyhow::{bail, Context, Result};
use fs_err as fs;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write as _;
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
    // https://peps.python.org/pep-0639/#license-file-multiple-use
    pub license_files: Vec<PathBuf>,
    pub classifiers: Vec<String>,
    pub requires_dist: Vec<String>,
    pub provides_dist: Vec<String>,
    pub obsoletes_dist: Vec<String>,
    pub requires_python: Option<String>,
    pub requires_external: Vec<String>,
    pub project_url: HashMap<String, String>,
    pub provides_extra: Vec<String>,
    pub scripts: HashMap<String, String>,
    pub gui_scripts: HashMap<String, String>,
    pub entry_points: HashMap<String, HashMap<String, String>>,
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

            if let Some(pyproject_toml::License { file, text }) = &project.license {
                if file.is_some() && text.is_some() {
                    bail!("file and text fields of 'project.license' are mutually-exclusive, only one of them should be specified");
                }
                if let Some(license_path) = file {
                    let license_path = pyproject_dir.join(license_path);
                    self.license_files.push(license_path);
                }
                if let Some(license_text) = text {
                    self.license = Some(license_text.clone());
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
                        println!("📦 Including license file \"{}\"", license_path.display());
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
                            emails.push(format!("{} <{}>", name, email));
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
                            emails.push(format!("{} <{}>", name, email));
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
                for (extra, deps) in dependencies {
                    self.provides_extra.push(extra.clone());
                    for dep in deps {
                        let dist = if let Some((dep, marker)) = dep.split_once(';') {
                            // optional dependency already has environment markers
                            let new_marker =
                                format!("({}) and extra == '{}'", marker.trim(), extra);
                            format!("{}; {}", dep, new_marker)
                        } else {
                            format!("{}; extra == '{}'", dep, extra)
                        };
                        self.requires_dist.push(dist);
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
        cargo_toml: &CargoToml,
        manifest_path: impl AsRef<Path>,
        cargo_metadata: &cargo_metadata::Metadata,
    ) -> Result<Metadata21> {
        let package = cargo_metadata
            .root_package()
            .context("Expected cargo to return metadata with root_package")?;
        let authors = package.authors.join(", ");

        let classifiers = cargo_toml.classifiers();

        let author_email = if authors.contains('@') {
            Some(authors.clone())
        } else {
            None
        };

        let extra_metadata = cargo_toml.remaining_core_metadata();

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

            description_content_type = extra_metadata
                .description_content_type
                .or_else(|| Some(path_to_content_type(&readme_path)));
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
        let name = extra_metadata
            .name
            .map(|name| {
                if let Some(pos) = name.find('.') {
                    name.split_at(pos).0.to_string()
                } else {
                    name.clone()
                }
            })
            .unwrap_or_else(|| package.name.clone());
        let mut project_url = extra_metadata.project_url.unwrap_or_default();
        if let Some(repository) = package.repository.as_ref() {
            project_url.insert("Source Code".to_string(), repository.clone());
        }

        let metadata = Metadata21 {
            metadata_version: "2.1".to_owned(),

            // Mapped from cargo metadata
            name,
            version: package.version.to_string(),
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
            license_files: Vec::new(),

            // Values provided through `[project.metadata.maturin]`
            classifiers,
            maintainer: extra_metadata.maintainer,
            maintainer_email: extra_metadata.maintainer_email,
            requires_dist: extra_metadata.requires_dist.unwrap_or_default(),
            requires_python: extra_metadata.requires_python,
            requires_external: extra_metadata.requires_external.unwrap_or_default(),
            project_url,
            provides_extra: extra_metadata.provides_extra.unwrap_or_default(),

            // Officially rarely used, and afaik not applicable with pyo3
            provides_dist: Vec::new(),
            obsoletes_dist: Vec::new(),

            // Open question: Should those also be supported? And if so, how?
            platform: Vec::new(),
            supported_platform: Vec::new(),
            scripts: cargo_toml.scripts(),
            gui_scripts: HashMap::new(),
            entry_points: HashMap::new(),
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
            ("Version", self.get_pep440_version()),
        ];

        let mut add_vec = |name, values: &[String]| {
            for i in values {
                fields.push((name, i.clone()));
            }
        };

        add_vec("Platform", &self.platform);
        add_vec("Supported-Platform", &self.supported_platform);
        add_vec("Classifier", &self.classifiers);
        add_vec("Requires-Dist", &self.requires_dist);
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
        add_option("Requires-Python", &self.requires_python);
        add_option("Description-Content-Type", &self.description_content_type);
        // Project-URL is special
        // "A string containing a browsable URL for the project and a label for it, separated by a comma."
        // `Project-URL: Bug Tracker, http://bitbucket.org/tarek/distribute/issues/`
        for (key, value) in self.project_url.iter() {
            fields.push(("Project-URL", format!("{}, {}", key, value)))
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
            writeln!(out, "{}: {}", key, value)?;
        }

        if let Some(body) = body {
            writeln!(out, "\n{}", body)?;
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
        self.get_pep440_version().replace('-', "_")
    }

    /// Returns the version encoded according to PEP 440
    ///
    /// See https://github.com/pypa/setuptools/blob/d90cf84e4890036adae403d25c8bb4ee97841bbf/pkg_resources/__init__.py#L1336-L1345
    pub fn get_pep440_version(&self) -> String {
        match pep440::Version::parse(&self.version) {
            Some(ver) => ver.normalize(),
            None => {
                let ver = self.version.replace(' ', ".");
                let re = Regex::new(r"[^A-Za-z0-9.]+").unwrap();
                re.replace_all(&ver, "-").to_string()
            }
        }
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
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    fn assert_metadata_from_cargo_toml(
        readme: &str,
        cargo_toml: &str,
        expected: &str,
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
        fs::write(&manifest_path, &toml_with_path).unwrap();

        let cargo_toml_struct: CargoToml = toml_edit::easy::from_str(&toml_with_path).unwrap();
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_path)
            .exec()
            .unwrap();

        let metadata =
            Metadata21::from_cargo_toml(&cargo_toml_struct, crate_path, &cargo_metadata).unwrap();

        let actual = metadata.to_file_contents().unwrap();

        assert_eq!(
            actual.trim(),
            expected.trim(),
            "Actual metadata differed from expected\nEXPECTED:\n{}\n\nGOT:\n{}",
            expected,
            actual
        );

        // get_dist_info_dir test checks against hard-coded values - check that they are as expected in the source first
        assert!(
            cargo_toml.contains("name = \"info-project\"")
                && cargo_toml.contains("version = \"0.1.0\""),
            "cargo_toml name and version string do not match hardcoded values, test will fail",
        );

        if cargo_toml_struct.remaining_core_metadata().name.is_none() {
            assert_eq!(
                metadata.get_dist_info_dir(),
                PathBuf::from("info_project-0.1.0.dist-info"),
                "Dist info dir differed from expected"
            );
        }
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

            [package.metadata.maturin.scripts]
            ph = "maturin:print_hello"

            [package.metadata.maturin]
            classifiers = ["Programming Language :: Python"]
            requires-dist = ["flask~=1.1.0", "toml==0.10.0"]
            project-url = { "Bug Tracker" = "http://bitbucket.org/tarek/distribute/issues/" }
        "#
        );

        let expected = indoc!(
            r#"
            Metadata-Version: 2.1
            Name: info-project
            Version: 0.1.0
            Classifier: Programming Language :: Python
            Requires-Dist: flask~=1.1.0
            Requires-Dist: toml==0.10.0
            Summary: A test project
            Keywords: ffi,test
            Home-Page: https://example.org
            Author: konstin <konstin@mailbox.org>
            Author-email: konstin <konstin@mailbox.org>
            Description-Content-Type: text/markdown; charset=UTF-8; variant=GFM
            Project-URL: Bug Tracker, http://bitbucket.org/tarek/distribute/issues/

            # Some test package

            This is the readme for a test package
        "#
        );

        assert_metadata_from_cargo_toml(readme, cargo_toml, expected);
    }

    #[test]
    fn test_metadata_from_cargo_toml_rst() {
        let readme = indoc!(
            r#"
            Some test package
            =================
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
            repository = "https://example.org"
            readme = "REPLACE_README_PATH"
            keywords = ["ffi", "test"]

            [lib]
            crate-type = ["cdylib"]
            name = "pyo3_pure"

            [package.metadata.maturin.scripts]
            ph = "maturin:print_hello"

            [package.metadata.maturin]
            classifiers = ["Programming Language :: Python"]
            requires-dist = ["flask~=1.1.0", "toml==0.10.0"]
            description-content-type = "text/x-rst"
        "#
        );

        let expected = indoc!(
            r#"
            Metadata-Version: 2.1
            Name: info-project
            Version: 0.1.0
            Classifier: Programming Language :: Python
            Requires-Dist: flask~=1.1.0
            Requires-Dist: toml==0.10.0
            Summary: A test project
            Keywords: ffi,test
            Home-Page: https://example.org
            Author: konstin <konstin@mailbox.org>
            Author-email: konstin <konstin@mailbox.org>
            Description-Content-Type: text/x-rst
            Project-URL: Source Code, https://example.org

            Some test package
            =================
        "#
        );

        assert_metadata_from_cargo_toml(readme, cargo_toml, expected);
    }

    #[test]
    fn test_metadata_from_cargo_toml_name_override() {
        let readme = indoc!(
            r#"
            Some test package
            =================
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

            [lib]
            crate-type = ["cdylib"]
            name = "pyo3_pure"

            [package.metadata.maturin.scripts]
            ph = "maturin:print_hello"

            [package.metadata.maturin]
            name = "info"
            classifiers = ["Programming Language :: Python"]
            description-content-type = "text/x-rst"
        "#
        );

        let expected = indoc!(
            r#"
            Metadata-Version: 2.1
            Name: info
            Version: 0.1.0
            Classifier: Programming Language :: Python
            Summary: A test project
            Home-Page: https://example.org
            Author: konstin <konstin@mailbox.org>
            Author-email: konstin <konstin@mailbox.org>
            Description-Content-Type: text/x-rst

            Some test package
            =================
        "#
        );

        let metadata = assert_metadata_from_cargo_toml(readme, cargo_toml, expected);

        assert_eq!(
            metadata.get_dist_info_dir(),
            PathBuf::from("info-0.1.0.dist-info"),
            "Dist info dir differed from expected"
        );
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
        let cargo_toml_str = fs_err::read_to_string(manifest_dir.join("Cargo.toml")).unwrap();
        let cargo_toml: CargoToml = toml_edit::easy::from_str(&cargo_toml_str).unwrap();
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_dir.join("Cargo.toml"))
            .exec()
            .unwrap();
        let mut metadata =
            Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir, &cargo_metadata).unwrap();
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
            Some(fs_err::read_to_string("test-crates/pyo3-pure/Readme.md").unwrap())
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
                "attrs; extra == 'test'",
                "boltons; (sys_platform == 'win32') and extra == 'test'"
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
        let cargo_toml_str = fs_err::read_to_string(manifest_dir.join("Cargo.toml")).unwrap();
        let cargo_toml: CargoToml = toml_edit::easy::from_str(&cargo_toml_str).unwrap();
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_dir.join("Cargo.toml"))
            .exec()
            .unwrap();
        let mut metadata =
            Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir, &cargo_metadata).unwrap();
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
        let cargo_toml_str = fs_err::read_to_string(manifest_dir.join("Cargo.toml")).unwrap();
        let cargo_toml = toml_edit::easy::from_str(&cargo_toml_str).unwrap();
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_dir.join("Cargo.toml"))
            .exec()
            .unwrap();
        let metadata =
            Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir, &cargo_metadata).unwrap();
        assert!(metadata.description.unwrap().starts_with("# pyo3-mixed"));
        assert_eq!(
            metadata.description_content_type.unwrap(),
            "text/markdown; charset=UTF-8; variant=GFM"
        );
    }

    #[test]
    fn test_merge_metadata_from_pyproject_dynamic_license_test() {
        let manifest_dir = PathBuf::from("test-crates").join("license-test");
        let cargo_toml_str = fs_err::read_to_string(&manifest_dir.join("Cargo.toml")).unwrap();
        let cargo_toml: CargoToml = toml_edit::easy::from_str(&cargo_toml_str).unwrap();
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_dir.join("Cargo.toml"))
            .exec()
            .unwrap();
        let mut metadata =
            Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir, &cargo_metadata).unwrap();
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
