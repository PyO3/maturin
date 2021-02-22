//! The uploading logic was mostly reverse engineered; I wrote it down as
//! documentation at https://warehouse.readthedocs.io/api-reference/legacy/#upload-api

use crate::Registry;
use fs_err::File;
use regex::Regex;
use reqwest::{self, blocking::multipart::Form, blocking::Client, StatusCode};
use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Error type for different types of errors that can happen when uploading a
/// wheel.
///
/// The most interesting type is AuthenticationError because it allows asking
/// the user to reenter the password
#[derive(Error, Debug)]
#[error("Uploading to the registry failed")]
pub enum UploadError {
    /// Any reqwest error
    #[error("Http error")]
    RewqestError(#[source] reqwest::Error),
    /// The registry returned a "403 Forbidden"
    #[error("Username or password are incorrect")]
    AuthenticationError,
    /// Reading the wheel failed
    #[error("IO Error")]
    IoError(#[source] io::Error),
    /// The registry returned something else than 200
    #[error("Failed to upload the wheel with status {0}: {1}")]
    StatusCodeError(String, String),
    /// File already exists
    #[error("File already exists: {0}")]
    FileExistsError(String),
    /// Read package metadata error
    #[error("Could not read the metadata from the package at {0}")]
    PkgInfoError(PathBuf, #[source] python_pkginfo::Error),
}

impl From<io::Error> for UploadError {
    fn from(error: io::Error) -> Self {
        UploadError::IoError(error)
    }
}

impl From<reqwest::Error> for UploadError {
    fn from(error: reqwest::Error) -> Self {
        UploadError::RewqestError(error)
    }
}

/// Port of pip's `canonicalize_name`
/// https://github.com/pypa/pip/blob/b33e791742570215f15663410c3ed987d2253d5b/src/pip/_vendor/packaging/utils.py#L18-L25
fn canonicalize_name(name: &str) -> String {
    Regex::new("[-_.]+")
        .unwrap()
        .replace(name, "-")
        .to_lowercase()
}

/// Uploads a single wheel to the registry
pub fn upload(registry: &Registry, wheel_path: &Path) -> Result<(), UploadError> {
    let mut wheel = File::open(&wheel_path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut wheel, &mut hasher)?;
    let hash_hex = format!("{:x}", hasher.finalize());

    let dist = python_pkginfo::Distribution::new(wheel_path)
        .map_err(|err| UploadError::PkgInfoError(wheel_path.to_owned(), err))?;
    let metadata = dist.metadata();

    let mut api_metadata = vec![
        (":action", "file_upload".to_string()),
        ("sha256_digest", hash_hex),
        ("protocol_version", "1".to_string()),
        ("metadata_version", metadata.metadata_version.clone()),
        ("name", canonicalize_name(&metadata.name)),
        ("version", metadata.version.clone()),
        ("pyversion", dist.python_version().to_string()),
        ("filetype", dist.r#type().to_string()),
    ];

    let mut add_option = |name, value: &Option<String>| {
        if let Some(some) = value.clone() {
            api_metadata.push((name, some));
        }
    };

    // https://github.com/pypa/warehouse/blob/75061540e6ab5aae3f8758b569e926b6355abea8/warehouse/forklift/legacy.py#L424
    add_option("summary", &metadata.summary);
    add_option("description", &metadata.description);
    add_option(
        "description_content_type",
        &metadata.description_content_type,
    );
    add_option("author", &metadata.author);
    add_option("author_email", &metadata.author_email);
    add_option("maintainer", &metadata.maintainer);
    add_option("maintainer_email", &metadata.maintainer_email);
    add_option("license", &metadata.license);
    add_option("keywords", &metadata.keywords);
    add_option("home_page", &metadata.home_page);
    add_option("download_url", &metadata.download_url);
    add_option("requires_python", &metadata.requires_python);
    add_option("summary", &metadata.summary);

    if metadata.requires_python.is_none() {
        // GitLab PyPI repository API implementation requires this metadata field
        // and twine always includes it in the request, even when it's empty.
        api_metadata.push(("requires_python", "".to_string()));
    }

    let mut add_vec = |name, values: &[String]| {
        for i in values {
            api_metadata.push((name, i.clone()));
        }
    };

    add_vec("classifiers", &metadata.classifiers);
    add_vec("platform", &metadata.platforms);
    add_vec("requires_dist", &metadata.requires_dist);
    add_vec("provides_dist", &metadata.provides_dist);
    add_vec("obsoletes_dist", &metadata.obsoletes_dist);
    add_vec("requires_external", &metadata.requires_external);
    add_vec("project_urls", &metadata.project_urls);

    let mut form = Form::new();
    for (key, value) in api_metadata {
        form = form.text(key, value);
    }

    form = form.file("content", &wheel_path)?;

    let client = Client::new();
    let response = client
        .post(registry.url.clone())
        .header(
            reqwest::header::USER_AGENT,
            format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        )
        .multipart(form)
        .basic_auth(registry.username.clone(), Some(registry.password.clone()))
        .send()?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let err_text = response.text().unwrap_or_else(|e| {
        format!(
            "The registry should return some text, even in case of an error, but didn't ({})",
            e
        )
    });
    // Detect FileExistsError the way twine does
    // https://github.com/pypa/twine/blob/87846e5777b380d4704704a69e1f9a7a1231451c/twine/commands/upload.py#L30
    if status == StatusCode::FORBIDDEN {
        if err_text.contains("overwrite artifact") {
            // Artifactory (https://jfrog.com/artifactory/)
            Err(UploadError::FileExistsError(err_text))
        } else {
            Err(UploadError::AuthenticationError)
        }
    } else {
        let status_string = status.to_string();
        if status == StatusCode::CONFLICT // pypiserver (https://pypi.org/project/pypiserver)
            // PyPI / TestPyPI
            || (status == StatusCode::BAD_REQUEST && err_text.contains("already exists"))
            // Nexus Repository OSS (https://www.sonatype.com/nexus-repository-oss)
            || (status == StatusCode::BAD_REQUEST && err_text.contains("updating asset"))
            // # Gitlab Enterprise Edition (https://about.gitlab.com)
            || (status == StatusCode::BAD_REQUEST && err_text.contains("already been taken"))
        {
            Err(UploadError::FileExistsError(err_text))
        } else {
            Err(UploadError::StatusCodeError(status_string, err_text))
        }
    }
}
