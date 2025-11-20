use crate::PyProjectToml;
use anyhow::{Context, Result, bail, format_err};
use fs_err as fs;
use indexmap::IndexMap;
use normpath::PathExt;
use pep440_rs::{Version, VersionSpecifiers};
use pep508_rs::{
    ExtraName, ExtraOperator, MarkerExpression, MarkerTree, MarkerValueExtra, Requirement,
};
use pyproject_toml::{License, check_pep639_glob};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::str;
use std::str::FromStr;
use tracing::debug;

/// The metadata required to generate the .dist-info directory
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct WheelMetadata {
    /// Python Package Metadata 2.4
    pub metadata24: Metadata24,
    /// The `[console_scripts]` for the entry_points.txt
    pub scripts: HashMap<String, String>,
    /// The name of the module can be distinct from the package name, mostly
    /// because package names normally contain minuses while module names
    /// have underscores. The package name is part of metadata24
    pub module_name: String,
}

/// Python Package Metadata 2.4 as specified in
/// https://packaging.python.org/specifications/core-metadata/
/// Maturin writes static metadata and does not support dynamic fields atm.
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
#[allow(missing_docs)]
pub struct Metadata24 {
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
    pub license_expression: Option<String>,
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

impl Metadata24 {
    /// Initializes with name, version and otherwise the defaults
    pub fn new(name: String, version: Version) -> Self {
        Self {
            metadata_version: "2.4".to_string(),
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
            license_expression: None,
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

impl Metadata24 {
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
            let dynamic: HashSet<&str> = project
                .dynamic
                .as_ref()
                .map(|x| x.iter().map(AsRef::as_ref).collect())
                .unwrap_or_default();
            if dynamic.contains("name") {
                bail!("`project.dynamic` must not specify `name` in pyproject.toml");
            }

            // According to PEP 621, build backends must not add metadata fields
            // that are not declared in the dynamic list. Clear fields from Cargo.toml
            // that are not in the dynamic list.
            if !dynamic.contains("description") {
                self.summary = None;
            }
            if !dynamic.contains("authors") {
                self.author = None;
                self.author_email = None;
            }
            if !dynamic.contains("maintainers") {
                self.maintainer = None;
                self.maintainer_email = None;
            }
            if !dynamic.contains("keywords") {
                self.keywords = None;
            }
            if !dynamic.contains("urls") {
                self.project_url.clear();
            }
            if !dynamic.contains("license") {
                self.license = None;
                // Don't clear license_files as they may come from auto-discovery
            }
            if !dynamic.contains("classifiers") {
                self.classifiers.clear();
            }
            if !dynamic.contains("readme") {
                self.description = None;
                self.description_content_type = None;
            }
            if !dynamic.contains("requires-python") {
                self.requires_python = None;
            }

            self.name.clone_from(&project.name);

            let version_ok = pyproject_toml.warn_invalid_version_info();
            if !version_ok {
                // This is a hard error for maturin>=2.0, see https://github.com/PyO3/maturin/issues/2416.
                let current_major = env!("CARGO_PKG_VERSION_MAJOR").parse::<usize>().unwrap();
                if current_major > 1 {
                    bail!("Invalid version information in pyproject.toml.");
                }
            }

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
                        bail!(
                            "file and text fields of 'project.readme' are mutually-exclusive, only one of them should be specified"
                        );
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
                    self.description_content_type.clone_from(content_type);
                }
                None => {}
            }

            // Make sure existing license files from `Cargo.toml` are relative to the project root
            let license_files = std::mem::take(&mut self.license_files);
            for license_file in license_files {
                let license_path = license_file
                    .strip_prefix(pyproject_dir)
                    .with_context(|| {
                        format!(
                            "license file `{}` exists outside of the project root `{}`",
                            license_file.display(),
                            pyproject_dir.display()
                        )
                    })?
                    .to_path_buf();
                self.license_files.push(license_path);
            }

            if let Some(requires_python) = &project.requires_python {
                self.requires_python = Some(requires_python.clone());
            }

            if let Some(license) = &project.license {
                match license {
                    // PEP 639
                    License::Spdx(license_expr) => {
                        self.license_expression = Some(license_expr.clone())
                    }
                    // Deprecated by PEP 639
                    License::File { file } => {
                        self.license_files.push(file.to_path_buf());
                    }
                    License::Text { text } => self.license = Some(text.clone()),
                }
            }

            // Handle PEP 639 license-files field
            if let Some(license_files) = &project.license_files {
                // Safe on Windows and Unix as neither forward nor backwards slashes are escaped.
                let escaped_pyproject_dir =
                    PathBuf::from(glob::Pattern::escape(pyproject_dir.to_str().unwrap()));
                for license_glob in license_files {
                    check_pep639_glob(license_glob)?;
                    for license_path in
                        glob::glob(&escaped_pyproject_dir.join(license_glob).to_string_lossy())?
                    {
                        let license_path = license_path?;
                        if !license_path.is_file() {
                            continue;
                        }
                        let license_path = license_path
                            .strip_prefix(pyproject_dir)
                            .expect("matched path starts with glob root")
                            .to_path_buf();
                        if !self.license_files.contains(&license_path) {
                            debug!("Including license file `{}`", license_path.display());
                            self.license_files.push(license_path);
                        }
                    }
                }
            } else {
                // Auto-discovery of license files for backwards compatibility
                // license-files.globs = ["LICEN[CS]E*", "COPYING*", "NOTICE*", "AUTHORS*"]
                let license_include_targets = ["LICEN[CS]E*", "COPYING*", "NOTICE*", "AUTHORS*"];
                let escaped_manifest_string =
                    glob::Pattern::escape(pyproject_dir.to_str().unwrap());
                let escaped_manifest_path = Path::new(&escaped_manifest_string);
                for pattern in license_include_targets.iter() {
                    for license_path in
                        glob::glob(&escaped_manifest_path.join(pattern).to_string_lossy())?
                            .filter_map(Result::ok)
                    {
                        if !license_path.is_file() {
                            continue;
                        }
                        let license_path = license_path
                            .strip_prefix(pyproject_dir)
                            .expect("matched path starts with glob root")
                            .to_path_buf();
                        // if the pyproject.toml specified the license file,
                        // then we won't list it as automatically included
                        if !self.license_files.contains(&license_path) {
                            eprintln!("📦 Including license file `{}`", license_path.display());
                            self.license_files.push(license_path);
                        }
                    }
                }
            }

            if let Some(authors) = &project.authors {
                let mut names = Vec::with_capacity(authors.len());
                let mut emails = Vec::with_capacity(authors.len());
                for author in authors {
                    match (author.name(), author.email()) {
                        (Some(name), Some(email)) => {
                            emails.push(escape_email_with_display_name(name, email));
                        }
                        (Some(name), None) => {
                            names.push(name);
                        }
                        (None, Some(email)) => {
                            emails.push(email.to_string());
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
                    match (maintainer.name(), maintainer.email()) {
                        (Some(name), Some(email)) => {
                            emails.push(escape_email_with_display_name(name, email));
                        }
                        (Some(name), None) => {
                            names.push(name);
                        }
                        (None, Some(email)) => {
                            emails.push(email.to_string());
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
                self.classifiers.clone_from(classifiers);
            }

            if let Some(urls) = &project.urls {
                self.project_url.clone_from(urls);
            }

            if let Some(dependencies) = &project.dependencies {
                self.requires_dist.clone_from(dependencies);
            }

            if let Some(dependencies) = &project.optional_dependencies {
                // Transform the extra -> deps map into the PEP 508 style `dep ; extras = ...` style
                for (extra, deps) in dependencies {
                    self.provides_extra.push(extra.clone());
                    for dep in deps {
                        let mut dep = dep.clone();
                        // Keep in sync with `develop()`!
                        let new_extra = MarkerExpression::Extra {
                            operator: ExtraOperator::Equal,
                            name: MarkerValueExtra::Extra(
                                ExtraName::new(extra.clone())
                                    .with_context(|| format!("invalid extra name: {extra}"))?,
                            ),
                        };
                        dep.marker.and(MarkerTree::expression(new_extra));
                        self.requires_dist.push(dep);
                    }
                }
            }

            if let Some(scripts) = &project.scripts {
                self.scripts.clone_from(scripts);
            }
            if let Some(gui_scripts) = &project.gui_scripts {
                self.gui_scripts.clone_from(gui_scripts);
            }
            if let Some(entry_points) = &project.entry_points {
                // Raise error on ambiguous entry points: https://www.python.org/dev/peps/pep-0621/#entry-points
                if entry_points.contains_key("console_scripts") {
                    bail!("console_scripts is not allowed in project.entry-points table");
                }
                if entry_points.contains_key("gui_scripts") {
                    bail!("gui_scripts is not allowed in project.entry-points table");
                }
                self.entry_points.clone_from(entry_points);
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
    ) -> Result<Metadata24> {
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
        if let Some(homepage) = package.homepage.as_ref() {
            project_url.insert("Homepage".to_string(), homepage.clone());
        }
        if let Some(documentation) = package.documentation.as_ref() {
            project_url.insert("Documentation".to_string(), documentation.clone());
        }
        if let Some(repository) = package.repository.as_ref() {
            project_url.insert("Source Code".to_string(), repository.clone());
        }
        let license_files = if let Some(license_file) = package.license_file.as_ref() {
            let license_path = manifest_path.as_ref().join(license_file).normalize()?;
            if !license_path.is_file() {
                bail!(
                    "license file `{license_file}` specified in `{}` is not a file",
                    manifest_path.as_ref().display()
                );
            }
            vec![license_path.into_path_buf()]
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
        let metadata = Metadata24 {
            // name, version and metadata_version are added through metadata24::new()
            // Mapped from cargo metadata
            summary: package.description.as_ref().map(|d| d.trim().into()),
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
            ..Metadata24::new(name.to_string(), version)
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
            // Use a portable path to ensure the metadata is consistent between Unix and Windows.
            .map(|path| path.display().to_string().replace("\\", "/"))
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
        // PEP 639
        add_option("License-Expression", &self.license_expression);
        // Deprecated by PEP 639
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

    /// Returns the name of the .data directory as defined in the wheel specification
    pub fn get_data_dir(&self) -> PathBuf {
        PathBuf::from(format!(
            "{}-{}.data",
            &self.get_distribution_escaped(),
            &self.get_version_escaped()
        ))
    }
}

/// Escape email addresses with display name if necessary
/// according to RFC 822 Section 3.3. "specials".
fn escape_email_with_display_name(display_name: &str, email: &str) -> String {
    if display_name.chars().any(|c| {
        matches!(
            c,
            '(' | ')' | '<' | '>' | '@' | ',' | ';' | ':' | '\\' | '"' | '.' | '[' | ']'
        )
    }) {
        return format!(
            "\"{}\" <{email}>",
            display_name.replace('\\', "\\\\").replace('\"', "\\\"")
        );
    }
    format!("{display_name} <{email}>")
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
mod tests {
    use super::*;
    use cargo_metadata::MetadataCommand;
    use expect_test::{Expect, expect};
    use indoc::indoc;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn assert_metadata_from_cargo_toml(
        readme: &str,
        cargo_toml: &str,
        expected: Expect,
    ) -> Metadata24 {
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

        let metadata = Metadata24::from_cargo_toml(crate_path, &cargo_metadata).unwrap();

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
            description = """
A test project
            """
            homepage = "https://example.org"
            readme = "REPLACE_README_PATH"
            keywords = ["ffi", "test"]

            [lib]
            crate-type = ["cdylib"]
            name = "pyo3_pure"
        "#
        );

        let expected = expect![[r#"
            Metadata-Version: 2.4
            Name: info-project
            Version: 0.1.0
            Summary: A test project
            Keywords: ffi,test
            Home-Page: https://example.org
            Author: konstin <konstin@mailbox.org>
            Author-email: konstin <konstin@mailbox.org>
            Description-Content-Type: text/markdown; charset=UTF-8; variant=GFM
            Project-URL: Homepage, https://example.org

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
        let mut metadata = Metadata24::from_cargo_toml(&manifest_dir, &cargo_metadata).unwrap();
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
        let mut metadata = Metadata24::from_cargo_toml(&manifest_dir, &cargo_metadata).unwrap();
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
        let metadata = Metadata24::from_cargo_toml(&manifest_dir, &cargo_metadata).unwrap();
        assert!(metadata.description.unwrap().starts_with("# pyo3-mixed"));
        assert_eq!(
            metadata.description_content_type.unwrap(),
            "text/markdown; charset=UTF-8; variant=GFM"
        );
    }

    #[test]
    fn test_pep639() {
        let manifest_dir = PathBuf::from("test-crates").join("pyo3-mixed");
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_dir.join("Cargo.toml"))
            .exec()
            .unwrap();
        let mut metadata = Metadata24::from_cargo_toml(&manifest_dir, &cargo_metadata).unwrap();
        let pyproject_toml = PyProjectToml::new(manifest_dir.join("pyproject.toml")).unwrap();
        metadata
            .merge_pyproject_toml(&manifest_dir, &pyproject_toml)
            .unwrap();

        assert_eq!(metadata.license_expression.as_ref().unwrap(), "MIT");
        assert_eq!(metadata.license.as_ref(), None);
    }

    #[test]
    fn test_merge_metadata_from_pyproject_dynamic_license_test() {
        let manifest_dir = PathBuf::from("test-crates").join("license-test");
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(manifest_dir.join("Cargo.toml"))
            .exec()
            .unwrap();
        let mut metadata = Metadata24::from_cargo_toml(&manifest_dir, &cargo_metadata).unwrap();
        let pyproject_toml = PyProjectToml::new(manifest_dir.join("pyproject.toml")).unwrap();
        metadata
            .merge_pyproject_toml(&manifest_dir, &pyproject_toml)
            .unwrap();

        // verify Cargo.toml value came through
        assert_eq!(metadata.license.as_ref().unwrap(), "MIT");

        // verify we have the total number of expected licenses
        assert_eq!(4, metadata.license_files.len());

        // Verify pyproject.toml license = {file = ...} worked
        assert_eq!(metadata.license_files[0], PathBuf::from("LICENCE.txt"));

        // Verify the default licenses were included
        assert_eq!(metadata.license_files[1], PathBuf::from("LICENSE"));
        assert_eq!(metadata.license_files[2], PathBuf::from("NOTICE.md"));
        assert_eq!(metadata.license_files[3], PathBuf::from("AUTHORS.txt"));
    }

    #[test]
    fn test_escape_email_with_display_name_without_special_characters() {
        let display_name = "Foo Bar !#$%&'*+-/=?^_`{|}~ 123";
        let email = "foobar-123@example.com";
        let result = escape_email_with_display_name(display_name, email);
        assert_eq!(
            result,
            "Foo Bar !#$%&'*+-/=?^_`{|}~ 123 <foobar-123@example.com>"
        );
    }

    #[test]
    fn test_escape_email_with_display_name_with_special_characters() {
        let tests = [
            ("Foo ( Bar", "\"Foo ( Bar\""),
            ("Foo ) Bar", "\"Foo ) Bar\""),
            ("Foo < Bar", "\"Foo < Bar\""),
            ("Foo > Bar", "\"Foo > Bar\""),
            ("Foo @ Bar", "\"Foo @ Bar\""),
            ("Foo , Bar", "\"Foo , Bar\""),
            ("Foo ; Bar", "\"Foo ; Bar\""),
            ("Foo : Bar", "\"Foo : Bar\""),
            ("Foo \\ Bar", "\"Foo \\\\ Bar\""),
            ("Foo \" Bar", "\"Foo \\\" Bar\""),
            ("Foo . Bar", "\"Foo . Bar\""),
            ("Foo [ Bar", "\"Foo [ Bar\""),
            ("Foo ] Bar", "\"Foo ] Bar\""),
            ("Foo ) Bar", "\"Foo ) Bar\""),
            ("Foo ) Bar", "\"Foo ) Bar\""),
            ("Foo, Bar", "\"Foo, Bar\""),
        ];
        for (display_name, expected_name) in tests {
            let email = "foobar-123@example.com";
            let result = escape_email_with_display_name(display_name, email);
            let expected = format!("{expected_name} <{email}>");
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn test_issue_2544_respect_pyproject_dynamic_without_dynamic_fields() {
        let temp_dir = TempDir::new().unwrap();
        let crate_path = temp_dir.path();
        let manifest_path = crate_path.join("Cargo.toml");
        let pyproject_path = crate_path.join("pyproject.toml");

        // Create basic src structure
        fs::create_dir(crate_path.join("src")).unwrap();
        fs::write(crate_path.join("src/lib.rs"), "").unwrap();

        // Write Cargo.toml with metadata that should NOT be included
        // because pyproject.toml doesn't declare them as dynamic
        let cargo_toml = indoc!(
            r#"
            [package]
            name = "test-package"
            version = "0.1.0"
            description = "Description from Cargo.toml - should not appear"
            authors = ["author from cargo.toml <author@example.com>"]
            keywords = ["cargo", "toml", "keyword"]
            repository = "https://github.com/example/repo"

            [lib]
            crate-type = ["cdylib"]
            "#
        );
        fs::write(&manifest_path, cargo_toml).unwrap();

        // Write pyproject.toml WITHOUT declaring the fields as dynamic
        let pyproject_toml_content = indoc!(
            r#"
            [build-system]
            requires = ["maturin>=1.0,<2.0"]
            build-backend = "maturin"

            [project]
            name = "test-package"
            version = "0.1.0"
            # Note: no description, authors, keywords, urls in dynamic list
            # dynamic = []  # Not specified, so defaults to empty
            "#
        );
        fs::write(&pyproject_path, pyproject_toml_content).unwrap();

        // Load metadata as maturin does
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(&manifest_path)
            .exec()
            .unwrap();
        let mut metadata = Metadata24::from_cargo_toml(crate_path, &cargo_metadata).unwrap();
        let pyproject_toml = PyProjectToml::new(&pyproject_path).unwrap();
        metadata
            .merge_pyproject_toml(crate_path, &pyproject_toml)
            .unwrap();

        assert_eq!(
            metadata.summary, None,
            "summary should be None when not in dynamic list"
        );
        assert_eq!(
            metadata.author, None,
            "author should be None when not in dynamic list"
        );
        assert_eq!(
            metadata.keywords, None,
            "keywords should be None when not in dynamic list"
        );
        assert!(
            metadata.project_url.is_empty(),
            "project_url should be empty when not in dynamic list"
        );
    }

    #[test]
    fn test_issue_2544_respect_pyproject_dynamic_with_dynamic_fields() {
        let temp_dir = TempDir::new().unwrap();
        let crate_path = temp_dir.path();
        let manifest_path = crate_path.join("Cargo.toml");
        let pyproject_path = crate_path.join("pyproject.toml");

        // Create basic src structure
        fs::create_dir(crate_path.join("src")).unwrap();
        fs::write(crate_path.join("src/lib.rs"), "").unwrap();

        // Write Cargo.toml with metadata
        let cargo_toml = indoc!(
            r#"
            [package]
            name = "test-package"
            version = "0.1.0"
            description = "Description from Cargo.toml - should appear"
            authors = ["author from cargo.toml <author@example.com>"]
            keywords = ["cargo", "toml", "keyword"]
            repository = "https://github.com/example/repo"

            [lib]
            crate-type = ["cdylib"]
            "#
        );
        fs::write(&manifest_path, cargo_toml).unwrap();

        // Write pyproject.toml WITH fields declared as dynamic
        let pyproject_toml_content = indoc!(
            r#"
            [build-system]
            requires = ["maturin>=1.0,<2.0"]
            build-backend = "maturin"

            [project]
            name = "test-package"
            version = "0.1.0"
            # Fields declared as dynamic - should come from Cargo.toml
            dynamic = ["description", "authors", "keywords", "urls"]
            "#
        );
        fs::write(&pyproject_path, pyproject_toml_content).unwrap();

        // Load metadata as maturin does
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(&manifest_path)
            .exec()
            .unwrap();
        let mut metadata = Metadata24::from_cargo_toml(crate_path, &cargo_metadata).unwrap();
        let pyproject_toml = PyProjectToml::new(&pyproject_path).unwrap();
        metadata
            .merge_pyproject_toml(crate_path, &pyproject_toml)
            .unwrap();

        // These fields SHOULD be set because they are in dynamic list
        assert_eq!(
            metadata.summary,
            Some("Description from Cargo.toml - should appear".to_string())
        );
        assert_eq!(
            metadata.author,
            Some("author from cargo.toml <author@example.com>".to_string())
        );
        assert_eq!(metadata.keywords, Some("cargo,toml,keyword".to_string()));
        assert_eq!(
            metadata.project_url.get("Source Code"),
            Some(&"https://github.com/example/repo".to_string())
        );
    }
}
