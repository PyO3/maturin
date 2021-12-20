//! The uploading logic was mostly reverse engineered; I wrote it down as
//! documentation at https://warehouse.readthedocs.io/api-reference/legacy/#upload-api

use crate::Registry;
use anyhow::{bail, Context, Result};
use bytesize::ByteSize;
use configparser::ini::Ini;
use fs_err as fs;
use fs_err::File;
use regex::Regex;
use reqwest::Url;
use reqwest::{self, blocking::multipart::Form, blocking::Client, StatusCode};
use sha2::{Digest, Sha256};
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use structopt::StructOpt;
use thiserror::Error;

/// An account with a registry, possibly incomplete
#[derive(Debug, StructOpt)]
pub struct PublishOpt {
    #[structopt(
        short = "r",
        long = "repository-url",
        default_value = "https://upload.pypi.org/legacy/"
    )]
    /// The url of registry where the wheels are uploaded to
    registry: String,
    #[structopt(short, long)]
    /// Username for pypi or your custom registry
    username: Option<String>,
    #[structopt(short, long)]
    /// Password for pypi or your custom registry. Note that you can also pass the password
    /// through MATURIN_PASSWORD
    password: Option<String>,
    /// Continue uploading files if one already exists.
    /// (Only valid when uploading to PyPI. Other implementations may not support this.)
    #[structopt(long = "skip-existing")]
    skip_existing: bool,
}

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

/// Returns the password and a bool that states whether to ask for re-entering the password
/// after a failed authentication
///
/// Precedence:
/// 1. MATURIN_PASSWORD
/// 2. keyring
/// 3. stdin
fn get_password(_username: &str) -> String {
    if let Ok(password) = env::var("MATURIN_PASSWORD") {
        return password;
    };

    #[cfg(feature = "keyring")]
    {
        let service = env!("CARGO_PKG_NAME");
        let keyring = keyring::Entry::new(service, _username);
        if let Ok(password) = keyring.get_password() {
            return password;
        };
    }

    rpassword::prompt_password_stdout("Please enter your password: ").unwrap_or_else(|_| {
        // So we need this fallback for pycharm on windows
        let mut password = String::new();
        io::stdin()
            .read_line(&mut password)
            .expect("Failed to read line");
        password.trim().to_string()
    })
}

fn get_username() -> String {
    println!("Please enter your username:");
    let mut line = String::new();
    io::stdin().read_line(&mut line).unwrap();
    line.trim().to_string()
}

fn load_pypirc() -> Ini {
    let mut config = Ini::new();
    if let Some(mut config_path) = dirs::home_dir() {
        config_path.push(".pypirc");
        if let Ok(pypirc) = fs::read_to_string(config_path.as_path()) {
            let _ = config.read(pypirc);
        }
    }
    config
}

fn load_pypi_cred_from_config(config: &Ini, registry_name: &str) -> Option<(String, String)> {
    if let (Some(username), Some(password)) = (
        config.get(registry_name, "username"),
        config.get(registry_name, "password"),
    ) {
        return Some((username, password));
    }
    None
}

fn resolve_pypi_cred(
    opt: &PublishOpt,
    config: &Ini,
    registry_name: Option<&str>,
) -> (String, String) {
    // API token from environment variable takes priority
    if let Ok(token) = env::var("MATURIN_PYPI_TOKEN") {
        return ("__token__".to_string(), token);
    }

    if let Some((username, password)) =
        registry_name.and_then(|name| load_pypi_cred_from_config(config, name))
    {
        println!("🔐 Using credential in pypirc for upload");
        return (username, password);
    }

    // fallback to username and password
    let username = opt.username.clone().unwrap_or_else(get_username);
    let password = match opt.password {
        Some(ref password) => password.clone(),
        None => get_password(&username),
    };

    (username, password)
}

/// Asks for username and password for a registry account where missing.
fn complete_registry(opt: &PublishOpt) -> Result<Registry> {
    // load creds from pypirc if found
    let pypirc = load_pypirc();
    let (register_name, registry_url) =
        if !opt.registry.starts_with("http://") && !opt.registry.starts_with("https://") {
            if let Some(url) = pypirc.get(&opt.registry, "repository") {
                (Some(opt.registry.as_str()), url)
            } else {
                bail!(
                    "Failed to get registry {} in .pypirc. \
                    Note: Your index didn't start with http:// or https://, \
                    which is required for non-pypirc indices.",
                    opt.registry
                );
            }
        } else if opt.registry == "https://upload.pypi.org/legacy/" {
            (Some("pypi"), opt.registry.clone())
        } else {
            (None, opt.registry.clone())
        };
    let (username, password) = resolve_pypi_cred(opt, &pypirc, register_name);
    let registry = Registry::new(username, password, Url::parse(&registry_url)?);

    Ok(registry)
}

/// Port of pip's `canonicalize_name`
/// https://github.com/pypa/pip/blob/b33e791742570215f15663410c3ed987d2253d5b/src/pip/_vendor/packaging/utils.py#L18-L25
fn canonicalize_name(name: &str) -> String {
    Regex::new("[-_.]+")
        .unwrap()
        .replace_all(name, "-")
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

/// Handles authentication/keyring integration and retrying of the publish subcommand
pub fn upload_ui(items: &[PathBuf], publish: &PublishOpt) -> Result<()> {
    let registry = complete_registry(publish)?;

    println!("🚀 Uploading {} packages", items.len());

    for i in items {
        let upload_result = upload(&registry, i);

        match upload_result {
            Ok(()) => (),
            Err(UploadError::AuthenticationError) => {
                println!("⛔ Username and/or password are wrong");

                #[cfg(feature = "keyring")]
                {
                    // Delete the wrong password from the keyring
                    let old_username = registry.username;
                    let keyring = keyring::Entry::new(env!("CARGO_PKG_NAME"), &old_username);
                    match keyring.delete_password() {
                        Ok(()) => {
                            println!("🔑 Removed wrong password from keyring")
                        }
                        Err(keyring::Error::NoEntry)
                        | Err(keyring::Error::NoStorageAccess(_))
                        | Err(keyring::Error::PlatformFailure(_)) => {}
                        Err(err) => {
                            eprintln!("⚠️ Warning: Failed to remove password from keyring: {}", err)
                        }
                    }
                }

                bail!("Username and/or password are wrong");
            }
            Err(err) => {
                let filename = i.file_name().unwrap_or_else(|| i.as_os_str());
                if let UploadError::FileExistsError(_) = err {
                    if publish.skip_existing {
                        println!(
                            "⚠️ Note: Skipping {:?} because it appears to already exist",
                            filename
                        );
                        continue;
                    }
                }
                let filesize = fs::metadata(&i)
                    .map(|x| ByteSize(x.len()).to_string())
                    .unwrap_or_else(|e| format!("Failed to get the filesize of {:?}: {}", &i, e));
                return Err(err)
                    .context(format!("💥 Failed to upload {:?} ({})", filename, filesize));
            }
        }
    }

    println!("✨ Packages uploaded successfully");

    #[cfg(feature = "keyring")]
    {
        // We know the password is correct, so we can save it in the keyring
        let username = registry.username.clone();
        let keyring = keyring::Entry::new(env!("CARGO_PKG_NAME"), &username);
        let password = registry.password;
        match keyring.set_password(&password) {
            Ok(())
            | Err(keyring::Error::NoStorageAccess(_))
            | Err(keyring::Error::PlatformFailure(_)) => {}
            Err(err) => {
                eprintln!(
                    "⚠️ Warning: Failed to store the password in the keyring: {:?}",
                    err
                );
            }
        }
    }

    Ok(())
}
