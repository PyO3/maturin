//! Build and publish crates with pyo3 bindings als python packages (CLI)
//!
//! Run with --help for usage information

extern crate core;
extern crate failure;
#[macro_use]
extern crate human_panic;
extern crate keyring;
extern crate pyo3_pack;
extern crate reqwest;
extern crate rpassword;
#[macro_use]
extern crate structopt;
extern crate target_info;

use failure::Error;
use keyring::{Keyring, KeyringError};
use pyo3_pack::{develop, upload_wheels, BuildContext, PythonInterpreter, Registry, UploadError};
use reqwest::Url;
use std::env;
use std::io;
use std::path::PathBuf;
use structopt::StructOpt;
use target_info::Target;

/// Precedence:
/// 1. PYO3_PACK_PASSWORD
/// 2. keyring
/// 3. stdin
fn get_password(username: &str) -> String {
    if let Ok(password) = env::var("PYO3_PACK_PASSWORD") {
        return password;
    };

    let service = env!("CARGO_PKG_NAME");
    let keyring = Keyring::new(&service, &username);
    if let Ok(password) = keyring.get_password() {
        return password;
    };

    rpassword::prompt_password_stdout("Please enter your password: ").unwrap()
}

fn get_username() -> String {
    println!("Please enter your username:");
    let mut line = String::new();
    io::stdin().read_line(&mut line).unwrap();
    line.trim().to_string()
}

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
    /// Password for pypi or your custom registry
    password: Option<String>,
}

#[derive(Debug, StructOpt)]
#[structopt(name = "pyo3-pack")]
/// Build and publish crates with pyo3 bindings as python packages
enum Opt {
    #[structopt(name = "build")]
    /// Build the crate into wheels
    Build {
        #[structopt(flatten)]
        build: BuildContext,
    },
    #[structopt(name = "publish")]
    /// Build and publish the crate as wheels to pypi
    Publish {
        #[structopt(flatten)]
        build: BuildContext,
        #[structopt(flatten)]
        publish: PublishOpt,
    },
    #[structopt(name = "list-python")]
    /// Searches and lists the available python installations
    ListPython,
    #[structopt(name = "develop")]
    /// Installs the crate as module in the current virtualenv so you can import it
    ///
    /// Note that this command doesn't create entrypoints
    Develop {
        /// The crate providing the python bindings
        #[structopt(short = "b", long = "bindings-crate", default_value = "pyo3")]
        binding_crate: String,
        #[structopt(
            short = "m",
            long = "manifest-path",
            parse(from_os_str),
            default_value = "Cargo.toml"
        )]
        /// The path to the Cargo.toml
        manifest_path: PathBuf,
        /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
        #[structopt(long = "cargo-extra-args")]
        cargo_extra_args: Vec<String>,
        /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
        #[structopt(long = "rustc-extra-args")]
        rustc_extra_args: Vec<String>,
    },
}

fn run() -> Result<(), Error> {
    let opt = Opt::from_args();

    match opt {
        Opt::Build { build } => {
            build.build_wheels()?;
        }
        Opt::Publish { build, publish } => {
            let (wheels, metadata) = build.build_wheels()?;

            let mut registry = complete_registry(&publish)?;

            loop {
                println!("Uploading {} packages", wheels.len());

                let result = upload_wheels(&registry, &wheels, &metadata);

                match result {
                    Ok(()) => {
                        println!("Packages uploaded succesfully");

                        // We know the password is correct, so we can save it in the keyring
                        let username = registry.username.clone();
                        let keyring = Keyring::new(&env!("CARGO_PKG_NAME"), &username);
                        let password = registry.password.clone();
                        keyring.set_password(&password).unwrap_or_else(|e| {
                            eprintln!("Failed to store the password in the keyring: {:?}", e)
                        });

                        return Ok(());
                    }
                    Err(UploadError::AuthenticationError) => {
                        println!("Username and/or password are wrong");
                        // Delete the wrong password from the keyring
                        let old_username = registry.username.clone();
                        let keyring = Keyring::new(&env!("CARGO_PKG_NAME"), &old_username);
                        match keyring.delete_password() {
                            Ok(()) => {}
                            Err(KeyringError::NoPasswordFound)
                            | Err(KeyringError::NoBackendFound) => {}
                            _ => eprintln!("Failed to remove password from keyring"),
                        }

                        let username = get_username();
                        let password =
                            rpassword::prompt_password_stdout("Please enter your password: ")
                                .unwrap();

                        registry = Registry::new(username, password, registry.url);
                        println!("Retrying")
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
        Opt::ListPython => {
            let pointer_width = match Target::pointer_width() {
                "32" => 32,
                "64" => 64,
                _ => panic!("{} is a pretty odd pointer width", Target::pointer_width()),
            };
            let found = PythonInterpreter::find_all(&Target::os(), pointer_width)?;
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
        } => {
            develop(
                binding_crate,
                manifest_path,
                cargo_extra_args,
                rustc_extra_args,
            )?;
        }
    }

    Ok(())
}

fn main() {
    setup_panic!();

    if let Err(e) = run() {
        for cause in e.as_fail().iter_chain().collect::<Vec<_>>().iter().rev() {
            println!("{}", cause);
        }
    }
}
