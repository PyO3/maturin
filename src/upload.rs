//! The uploading logic was mostly reverse engineered; I wrote it down as
//! documentation at https://warehouse.readthedocs.io/api-reference/legacy/#upload-api

use crate::Metadata21;
use crate::Registry;
use curl::easy::{Easy, Form, List};
use failure::Fail;
use log::log_enabled;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::path::Path;
use std::str;

/// Error type for different types of errors that can happen when uploading a
/// wheel.
///
/// The most interesting tpye is AuthenticationError because it allows asking
/// the user to reenter the password
#[derive(Fail, Debug)]
#[fail(display = "Uploading to the registry failed")]
pub enum UploadError {
    /// Any curl error
    #[fail(display = "Http error")]
    CurlError(#[cause] curl::Error),
    /// Any curl form error
    #[fail(display = "Form error")]
    FormError(#[cause] curl::FormError),
    /// The registry returned a "403 Forbidden"
    #[fail(display = "Username or password are incorrect")]
    AuthenticationError,
    /// Reading the wheel failed
    #[fail(display = "IO Error")]
    IOError(#[cause] io::Error),
    /// The registry returned something else than 200
    #[fail(display = "Failed to upload the wheel with status {}: {}", _0, _1)]
    StatusCodeError(u32, String),
}

impl From<io::Error> for UploadError {
    fn from(error: io::Error) -> Self {
        UploadError::IOError(error)
    }
}

impl From<curl::Error> for UploadError {
    fn from(error: curl::Error) -> Self {
        UploadError::CurlError(error)
    }
}

impl From<curl::FormError> for UploadError {
    fn from(error: curl::FormError) -> Self {
        UploadError::FormError(error)
    }
}

/// Scoping the usage of the easy client
fn curl_scoped(
    registry: &Registry,
    wheel_path: &Path,
    joined_metadata: Vec<(String, String)>,
) -> Result<(u32, Vec<u8>), UploadError> {
    let mut easy = Easy::new();

    // Request url
    easy.url(&registry.url)?;

    if log_enabled!(log::Level::Trace) {
        easy.verbose(true)?;
    }

    // Follow redirects
    easy.follow_location(true)?;
    let rc = unsafe {
        // POST again after following redirect
        // https://curl.haxx.se/libcurl/c/CURLOPT_POSTREDIR.html
        // https://github.com/curl/curl/blob/dca6f73613d8b578687bd4aeeedd198f9644bb53/include/curl/curl.h#L2055-L2065
        curl_sys::curl_easy_setopt(easy.raw(), curl_sys::CURLOPT_POSTREDIR, 1 | 2 | 4)
    };
    if rc != curl_sys::CURLE_OK {
        panic!("Failed to set CURLOPT_POSTREDIR");
    }

    // HTTP header
    let mut list = List::new();
    list.append(&format!(
        "User-Agent: {}/{}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    ))?;
    easy.http_headers(list)?;

    // Auth
    easy.username(&registry.username)?;
    easy.password(&registry.password)?;

    // HTTP body
    let mut form = Form::new();
    for (key, value) in joined_metadata {
        form.part(&key).contents(value.as_bytes()).add()?;
    }
    form.part("content").file(&wheel_path).add()?;
    easy.httppost(form)?;

    let mut data = Vec::new();
    // Collect the reponse
    {
        let mut transfer = easy.transfer();
        transfer.write_function(|new_data| {
            data.extend_from_slice(new_data);
            Ok(new_data.len())
        })?;
        // Actually upload
        transfer.perform()?;
    }
    let status_code = easy.response_code()?;
    Ok((status_code, data))
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
        .chain(metadata21.to_vec().clone().into_iter())
        // All fields must be lower case and with underscores or they will be ignored by warehouse
        .map(|(key, value)| (key.to_lowercase().replace("-", "_"), value))
        .collect();

    let (status_code, data) = curl_scoped(&registry, &wheel_path, joined_metadata)?;

    if status_code == 200 {
        Ok(())
    } else if status_code == 403 {
        Err(UploadError::AuthenticationError)
    } else {
        let err_text = str::from_utf8(&data)
            .map(ToString::to_string)
            .unwrap_or_else(|e| {
                format!(
                    "The registry should return some text, even in case of an error, but didn't ({})",
                    e
                )
            });
        Err(UploadError::StatusCodeError(status_code, err_text))
    }
}
