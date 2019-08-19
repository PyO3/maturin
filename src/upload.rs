//! The uploading logic was mostly reverse engineered; I wrote it down as
//! documentation at https://warehouse.readthedocs.io/api-reference/legacy/#upload-api

use crate::Metadata21;
use crate::Registry;
use failure::Fail;
use multipart::client::lazy::{LazyIoError, Multipart};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::io::Read;
use std::path::Path;

/// Error type for different types of errors that can happen when uploading a
/// wheel.
///
/// The most interesting tpye is AuthenticationError because it allows asking
/// the user to reenter the password
#[derive(Fail, Debug)]
#[fail(display = "Uploading to the registry failed")]
pub enum UploadError {
    /// The registry returned a "403 Forbidden"
    #[fail(display = "Username or password are incorrect")]
    AuthenticationError,
    /// Reading the wheel failed
    #[fail(display = "IO Error")]
    IOError(#[cause] io::Error),
    /// The registry returned something else than 200
    #[fail(display = "Failed to upload the wheel with status {}: {}", _0, _1)]
    StatusCodeError(u16, String),
    /// Error that occurs when reading a file part of the multipart fails
    #[fail(display = "Failed to read file to the '{:?}' field", _0)]
    MulitpartIOError(Option<String>, #[cause] io::Error),
    /// Error in the custom follow redirects implementation
    #[fail(display = "Uploading failed due to a http redirect error: {}", _0)]
    RedirectError(String),
}

impl From<io::Error> for UploadError {
    fn from(error: io::Error) -> Self {
        UploadError::IOError(error)
    }
}

impl From<LazyIoError<'_>> for UploadError {
    fn from(error: LazyIoError<'_>) -> Self {
        UploadError::MulitpartIOError(error.field_name.map(|x| x.to_string()), error.error)
    }
}

/// Uploads a single wheel to the registry
pub fn upload(
    registry: &Registry,
    wheel_path: &Path,
    metadata21: &Metadata21,
    supported_version: &str,
) -> Result<(), UploadError> {
    let mut wheel = File::open(&wheel_path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut wheel, &mut hasher)?;
    let hash_hex = format!("{:x}", hasher.result());

    let mut api_metadata = vec![
        (":action", "file_upload"),
        ("sha256_digest", &hash_hex),
        ("protocol_version", "1"),
    ];

    api_metadata.push(("pyversion", supported_version));

    if supported_version != "source" {
        api_metadata.push(("filetype", "bdist_wheel"));
    } else {
        api_metadata.push(("filetype", "sdist"));
    }

    let mut form = Multipart::new();
    for (key, value) in api_metadata {
        form.add_text(key, value.to_owned());
    }
    for (key, value) in metadata21.to_vec() {
        // All fields must be lower case and with underscores or they will be ignored by warehouse
        form.add_text(key.to_lowercase().replace("-", "_"), value.to_owned());
    }

    form.add_file("content", wheel_path.to_owned());
    let prepared_fields = form.prepare()?;
    let boundary = prepared_fields.boundary().to_string();
    let payload = prepared_fields.bytes().collect::<Result<Vec<u8>, _>>()?;

    let user_agent = format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    // Custom follow redirects implementation because the default one is broken
    // This should be reproduced and reported to ureq
    let mut retries = 0;
    let mut url = registry.url.clone();
    let response = loop {
        let response = ureq::post(&url)
            .set(
                "Content-Type",
                &format!(r#"multipart/form-data;boundary="{}""#, &boundary),
            )
            .set("User-Agent", &user_agent)
            .redirects(0)
            .auth(&registry.username.clone(), &registry.password.clone())
            .send_bytes(&payload);

        if response.status() == 301 {
            url = response
                .header("Location")
                .ok_or_else(|| {
                    UploadError::RedirectError("A redirect must have a new Location".to_string())
                })?
                .to_string();
            println!("➡️  Redirected to {}", url);
            if retries > 5 {
                return Err(UploadError::RedirectError("Too many redirects".to_string()));
            }
        } else {
            break response;
        }

        retries += 1;
    };
    if response.ok() {
        Ok(())
    } else if response.status() == 403 {
        // We assume that this means the password is wrong
        Err(UploadError::AuthenticationError)
    } else {
        let status_text = response.status();
        let err_text = response.into_string().unwrap_or_else(|e| {
            format!(
                "The registry should return some text, even in case of an error, but didn't ({})",
                e
            )
        });
        Err(UploadError::StatusCodeError(status_text, err_text))
    }
}
