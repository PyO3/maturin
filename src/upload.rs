//! The uploading logic was mostly reverse engineered; I wrote it down as
//! documentation at https://warehouse.readthedocs.io/api-reference/legacy/#upload-api

use reqwest::{self, header::ContentType, multipart::Form, Client, StatusCode};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use Metadata21;
use PythonInterpreter;
use Registry;

/// Error type for different types of errors that can happen when uploading a
/// wheel.
///
/// The most interesting tpye is AuthenticationError because it allows asking
/// the user to reenter the password
#[derive(Fail, Debug)]
#[fail(display = "Uploading to the registry failed")]
pub enum UploadError {
    /// Any reqwest error
    #[fail(display = "{}", _0)]
    RewqestError(#[cause] reqwest::Error),
    /// The registry returned a "403 Forbidden"
    #[fail(display = "Username or password are incorrect")]
    AuthenticationError,
    /// Reading the wheel failed
    #[fail(display = "{}", _0)]
    IOError(#[cause] io::Error),
    /// The registry returned something else than 200
    #[fail(display = "Failed to upload the wheel {}", _0)]
    StatusCodeError(String),
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

/// Uploads all given wheels to the registry
pub fn upload_wheels(
    regitry: &Registry,
    wheels: &[(PathBuf, Option<PythonInterpreter>)],
    metadata21: &Metadata21,
) -> Result<(), UploadError> {
    for (wheel_path, python_version) in wheels {
        upload(&regitry, &wheel_path, &metadata21, &python_version)?;
    }

    Ok(())
}

/// Uploads a single wheel to the registry
pub fn upload(
    registry: &Registry,
    wheel_path: &Path,
    metadata21: &Metadata21,
    python_version: &Option<PythonInterpreter>,
) -> Result<(), UploadError> {
    let mut wheel = File::open(&wheel_path)?;
    let hash = format!("{:x}", Sha256::digest_reader(&mut wheel)?);

    let mut api_metadata = vec![
        (":action".to_string(), "file_upload".to_string()),
        ("sha256_digest".to_string(), hash),
        ("protocol_version".to_string(), "1".to_string()),
    ];

    if let Some(python_version) = python_version {
        let pyversion = format!("cp{}{}", python_version.major, python_version.minor);
        api_metadata.push(("pyversion".to_string(), pyversion));
        api_metadata.push(("filetype".to_string(), "bdist_wheel".to_string()));
    } else {
        api_metadata.push(("pyversion".to_string(), "source".to_string()));
        api_metadata.push(("filetype".to_string(), "sdist".to_string()));
    }

    let joined_metadata: Vec<(String, String)> = api_metadata
        .into_iter()
        .chain(metadata21.to_vec().clone().into_iter())
        // All fields must be lower case and with underscores or they will be ignored by warehouse
        .map(|(key, value)| (key.to_lowercase().replace("-", "_"), value))
        .collect();

    let mut form = Form::new();
    for (key, value) in joined_metadata {
        form = form.text(key, value.to_owned())
    }

    form = form.file("content", &wheel_path)?;

    let client = Client::new();
    let mut response = client
        .post(registry.url.clone())
        .header(ContentType::json())
        .multipart(form)
        .basic_auth(registry.username.clone(), Some(registry.password.clone()))
        .send()?;

    if response.status().is_success() {
        Ok(())
    } else if response.status() == StatusCode::Forbidden {
        Err(UploadError::AuthenticationError)
    } else {
        let err_text = response.text().unwrap_or_else(|e| {
            format!(
                "The registry should return some text, even in case of an error, but didn't ({})",
                e
            )
        });
        Err(UploadError::StatusCodeError(err_text))
    }
}
