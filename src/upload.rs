//! The uploading logic was mostly reverse engineered; I wrote it down as
//! documentation at https://warehouse.readthedocs.io/api-reference/legacy/#upload-api

use crate::Registry;
use fs_err::File;
use reqwest::{self, blocking::multipart::Form, blocking::Client, StatusCode};
use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;
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
    IOError(#[source] io::Error),
    /// The registry returned something else than 200
    #[error("Failed to upload the wheel with status {0}: {1}")]
    StatusCodeError(String, String),
}

impl From<io::Error> for UploadError {
    fn from(error: io::Error) -> Self {
        UploadError::IOError(error)
    }
}

impl From<reqwest::Error> for UploadError {
    fn from(error: reqwest::Error) -> Self {
        UploadError::RewqestError(error)
    }
}

/// Uploads a single wheel to the registry
pub fn upload(
    registry: &Registry,
    wheel_path: &Path,
    metadata21: &[(String, String)],
    supported_version: &str,
) -> Result<(), UploadError> {
    let mut wheel = File::open(&wheel_path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut wheel, &mut hasher)?;
    let hash_hex = format!("{:x}", hasher.finalize());

    let mut api_metadata = vec![
        (":action".to_string(), "file_upload".to_string()),
        ("sha256_digest".to_string(), hash_hex),
        ("protocol_version".to_string(), "1".to_string()),
    ];

    api_metadata.push(("pyversion".to_string(), supported_version.to_string()));

    if supported_version != "source" {
        api_metadata.push(("filetype".to_string(), "bdist_wheel".to_string()));
    } else {
        api_metadata.push(("filetype".to_string(), "sdist".to_string()));
    }

    let joined_metadata: Vec<(String, String)> = api_metadata
        .into_iter()
        // Type system shenanigans
        .chain(metadata21.to_vec().into_iter())
        // All fields must be lower case and with underscores or they will be ignored by warehouse
        .map(|(key, value)| (key.to_lowercase().replace("-", "_"), value))
        .collect();

    let mut form = Form::new();
    for (key, value) in joined_metadata {
        form = form.text(key, value.to_owned())
    }

    form = form.file("content", &wheel_path)?;

    let client = Client::new();
    let response = client
        .post(registry.url.clone())
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )
        .header(
            reqwest::header::USER_AGENT,
            format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        )
        .multipart(form)
        .basic_auth(registry.username.clone(), Some(registry.password.clone()))
        .send()?;

    if response.status().is_success() {
        Ok(())
    } else if response.status() == StatusCode::FORBIDDEN {
        Err(UploadError::AuthenticationError)
    } else {
        let status_string = response.status().to_string();
        let err_text = response.text().unwrap_or_else(|e| {
            format!(
                "The registry should return some text, even in case of an error, but didn't ({})",
                e
            )
        });
        Err(UploadError::StatusCodeError(status_string, err_text))
    }
}
