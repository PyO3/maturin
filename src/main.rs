//! Build and publish crates with pyo3, rust-cpython and cffi bindings as well as rust binaries
//! as python packages. This file contains the CLI and keyring integration.
//!
//! Run with --help for usage information

#![cfg_attr(feature = "cargo-clippy", feature(tool_lints))]

extern crate core;
#[macro_use]
extern crate failure;
#[allow(unused_imports)]
#[macro_use]
extern crate human_panic;
#[cfg(feature = "keyring")]
extern crate keyring;
extern crate pyo3_pack;
#[cfg(feature = "upload")]
extern crate reqwest;
extern crate rpassword;
#[allow(unused_imports)]
#[macro_use]
extern crate structopt;
#[cfg(feature = "log")]
extern crate pretty_env_logger;

use failure::Error;
#[cfg(all(feature = "upload", feature = "keyring"))]
use keyring::{Keyring, KeyringError};
use pyo3_pack::{develop, BuildOptions, PythonInterpreter, Target};
#[cfg(feature = "upload")]
use pyo3_pack::{upload_wheels, Registry, UploadError};
#[cfg(feature = "upload")]
use reqwest::Url;
use std::env;
#[cfg(feature = "upload")]
use std::io;
use std::path::PathBuf;
use structopt::StructOpt;

/// Precedence:
/// 1. PYO3_PACK_PASSWORD
/// 2. keyring
/// 3. stdin
#[cfg(feature = "upload")]
fn get_password(_username: &str) -> String {
    if let Ok(password) = env::var("PYO3_PACK_PASSWORD") {
        return password;
    };

    #[cfg(feature = "keyring")]
    {
        let service = env!("CARGO_PKG_NAME");
        let keyring = Keyring::new(&service, &_username);
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

#[cfg(feature = "upload")]
fn get_username() -> String {
    println!("Please enter your username:");
    let mut line = String::new();
    io::stdin().read_line(&mut line).unwrap();
    line.trim().to_string()
}

#[cfg(feature = "upload")]
/// Asks for username and password for a registry account where missing.
fn complete_registry(opt: &PublishOpt) -> Result<Registry, Error> {
    let username = opt.username.clone().unwrap_or_else(get_username);
    let password = opt
        .password
        .clone()
        .unwrap_or_else(|| get_password(&username));

    Ok(Registry::new(
        username,
        password,
        Url::parse(&opt.registry)?,
    ))
}

/// An account with a registry, possibly incomplete
#[derive(Debug, StructOpt)]
struct PublishOpt {
    #[structopt(
        short = "r",
        long = "repository-url",
        default_value = "https://upload.pypi.org/legacy/"
    )]
    /// The url of registry where the wheels are uploaded to
    registry: String,
    #[structopt(short = "u", long = "username")]
    /// Username for pypi or your custom registry
    username: Option<String>,
    #[structopt(short = "p", long = "password")]
    /// Password for pypi or your custom registry. Note that you can also pass the password
    /// through PYO3_PACK_PASSWORD
    password: Option<String>,
}

#[derive(Debug, StructOpt)]
#[structopt(name = "pyo3-pack")]
#[cfg_attr(feature = "cargo-clippy", allow(clippy::large_enum_variant))]
/// Build and publish crates with pyo3 bindings as python packages
enum Opt {
    #[structopt(name = "build")]
    /// Build the crate into wheels
    Build {
        #[structopt(flatten)]
        build: BuildOptions,
    },
    #[cfg(feature = "upload")]
    #[structopt(name = "publish")]
    /// Build and publish the crate as wheels to pypi
    Publish {
        #[structopt(flatten)]
        build: BuildOptions,
        #[structopt(flatten)]
        publish: PublishOpt,
    },
    #[structopt(name = "list-python")]
    /// Searches and lists the available python installations
    ListPython,
    #[structopt(name = "develop")]
    /// Installs the crate as module in the current virtualenv so you can import it
    ///
    /// Note that this command doesn't create entrypoints and compiles in debug mode by default
    Develop {
        /// The crate providing the python bindings. pyo3, rust-cpython and cffi are supported
        #[structopt(short = "b", long = "bindings-crate")]
        binding_crate: Option<String>,
        #[structopt(
            short = "m",
            long = "manifest-path",
            parse(from_os_str),
            default_value = "Cargo.toml"
        )]
        /// The path to the Cargo.toml
        manifest_path: PathBuf,
        /// Pass --release to cargo
        #[structopt(long = "release")]
        release: bool,
        /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
        #[structopt(long = "cargo-extra-args")]
        cargo_extra_args: Vec<String>,
        /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
        #[structopt(long = "rustc-extra-args")]
        rustc_extra_args: Vec<String>,
    },
}

fn run() -> Result<(), Error> {
    #[cfg(feature = "log")]
    pretty_env_logger::init();

    let opt = Opt::from_args();

    match opt {
        Opt::Build { build } => {
            build.into_build_context()?.build_wheels()?;
        }
        #[cfg(feature = "upload")]
        Opt::Publish { build, publish } => {
            let build_context = build.into_build_context()?;

            if !build_context.release {
                eprintln!("Warning: You're publishing debug wheels");
            }

            let wheels = build_context.build_wheels()?;

            let mut registry = complete_registry(&publish)?;

            loop {
                println!("Uploading {} packages", wheels.len());

                let result = upload_wheels(&registry, &wheels, &build_context.metadata21);

                match result {
                    Ok(()) => {
                        println!("Packages uploaded succesfully");

                        #[cfg(feature = "keyring")]
                        {
                            // We know the password is correct, so we can save it in the keyring
                            let username = registry.username.clone();
                            let keyring = Keyring::new(&env!("CARGO_PKG_NAME"), &username);
                            let password = registry.password.clone();
                            keyring.set_password(&password).unwrap_or_else(|e| {
                                eprintln!("Failed to store the password in the keyring: {:?}", e)
                            });
                        }

                        return Ok(());
                    }
                    Err(UploadError::AuthenticationError) => {
                        println!("Username and/or password are wrong");

                        #[cfg(feature = "keyring")]
                        {
                            // Delete the wrong password from the keyring
                            let old_username = registry.username.clone();
                            let keyring = Keyring::new(&env!("CARGO_PKG_NAME"), &old_username);
                            match keyring.delete_password() {
                                Ok(()) => {}
                                Err(KeyringError::NoPasswordFound)
                                | Err(KeyringError::NoBackendFound) => {}
                                _ => eprintln!("Failed to remove password from keyring"),
                            }
                        }

                        let username = get_username();
                        let password =
                            rpassword::prompt_password_stdout("Please enter your password: ")
                                .unwrap_or_else(|_| {
                                    // So we need this fallback for pycharm on windows
                                    let mut password = String::new();
                                    io::stdin()
                                        .read_line(&mut password)
                                        .expect("Failed to read line");
                                    password.trim().to_string()
                                });

                        registry = Registry::new(username, password, registry.url);
                        println!("Retrying")
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
        Opt::ListPython => {
            let target = Target::current();
            let found = PythonInterpreter::find_all(&target)?;
            println!("{} python interpreter found:", found.len());
            for interpreter in found {
                println!(" - {}", interpreter);
            }
        }
        Opt::Develop {
            binding_crate,
            manifest_path,
            cargo_extra_args,
            rustc_extra_args,
            release,
        } => {
            let venv_dir = match env::var_os("VIRTUAL_ENV") {
                Some(dir) => PathBuf::from(dir),
                None => {
                    bail!("You need be inside a virtualenv to use develop (VIRTUAL_ENV isn't set)")
                }
            };

            develop(
                binding_crate,
                &manifest_path,
                cargo_extra_args,
                rustc_extra_args,
                &venv_dir,
                release,
            )?;
        }
    }

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        for cause in e.as_fail().iter_chain().collect::<Vec<_>>().iter().rev() {
            println!("{}", cause);
        }
        std::process::exit(1);
    }
}
