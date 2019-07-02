//! Build and publish crates with pyo3, rust-cpython and cffi bindings as well as rust binaries
//! as python packages. This file contains the CLI and keyring integration.
//!
//! Run with --help for usage information

use failure::{ResultExt, bail, Error, format_err};
#[cfg(feature = "human-panic")]
use human_panic::setup_panic;
#[cfg(feature = "password-storage")]
use keyring::{Keyring, KeyringError};
#[cfg(feature = "log")]
use pretty_env_logger;
use pyo3_pack::{
    develop, source_distribution, write_dist_info, BridgeModel, BuildOptions, CargoToml,
    Metadata21, PathWriter, PythonInterpreter, Target, get_pyproject_toml,
};
use std::path::PathBuf;
use std::{env, fs};
use structopt::StructOpt;
#[cfg(feature = "upload")]
use {
    pyo3_pack::{upload, Registry, UploadError},
    reqwest::Url,
    rpassword,
    std::io,
};
use cargo_metadata::MetadataCommand;

/// Returns the password and a bool that states whether to ask for re-entering the password
/// after a failed authentication
///
/// Precedence:
/// 1. PYO3_PACK_PASSWORD
/// 2. keyring
/// 3. stdin
#[cfg(feature = "upload")]
fn get_password(_username: &str) -> (String, bool) {
    if let Ok(password) = env::var("PYO3_PACK_PASSWORD") {
        return (password, false);
    };

    #[cfg(feature = "keyring")]
    {
        let service = env!("CARGO_PKG_NAME");
        let keyring = Keyring::new(&service, &_username);
        if let Ok(password) = keyring.get_password() {
            return (password, true);
        };
    }

    let password = rpassword::prompt_password_stdout("Please enter your password: ")
        .unwrap_or_else(|_| {
            // So we need this fallback for pycharm on windows
            let mut password = String::new();
            io::stdin()
                .read_line(&mut password)
                .expect("Failed to read line");
            password.trim().to_string()
        });

    (password, true)
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
fn complete_registry(opt: &PublishOpt) -> Result<(Registry, bool), Error> {
    let username = opt.username.clone().unwrap_or_else(get_username);
    let (password, reenter) = match opt.password {
        Some(ref password) => (password.clone(), false),
        None => get_password(&username),
    };

    let registry = Registry::new(username, password, Url::parse(&opt.registry)?);

    Ok((registry, reenter))
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
    #[structopt(short, long)]
    /// Username for pypi or your custom registry
    username: Option<String>,
    #[structopt(short, long)]
    /// Password for pypi or your custom registry. Note that you can also pass the password
    /// through PYO3_PACK_PASSWORD
    password: Option<String>,
    /// Do not pass --release to cargo
    #[structopt(long)]
    debug: bool,
    /// Strip the library for minimum file size
    #[structopt(long = "no-strip")]
    no_strip: bool,
}

#[derive(Debug, StructOpt)]
#[structopt(name = "pyo3-pack")]
#[cfg_attr(feature = "cargo-clippy", allow(clippy::large_enum_variant))]
/// Build and publish crates with pyo3, rust-cpython and cffi bindings as well
/// as rust binaries as python packages
enum Opt {
    #[structopt(name = "build")]
    /// Build the crate into python packages
    Build {
        #[structopt(flatten)]
        build: BuildOptions,
        /// Pass --release to cargo
        #[structopt(long)]
        release: bool,
        /// Strip the library for minimum file size
        #[structopt(long)]
        strip: bool,
        /// Don't build a source distribution
        #[structopt(long = "no-sdist")]
        no_sdist: bool,
    },
    #[cfg(feature = "upload")]
    #[structopt(name = "publish")]
    /// Build and publish the crate as python packages to pypi
    Publish {
        #[structopt(flatten)]
        build: BuildOptions,
        #[structopt(flatten)]
        publish: PublishOpt,
        /// Don't build a source distribution
        #[structopt(long = "no-sdist")]
        no_sdist: bool,
    },
    #[structopt(name = "list-python")]
    /// Searches and lists the available python installations
    ListPython,
    #[structopt(name = "develop")]
    /// Installs the crate as module in the current virtualenv
    ///
    /// Note that this command doesn't create entrypoints
    Develop {
        /// Which kind of bindings to use. Possible values are pyo3, rust-cpython, cffi and bin
        #[structopt(short = "b", long = "binding-crate")]
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
        #[structopt(long)]
        release: bool,
        /// Strip the library for minimum file size
        #[structopt(long)]
        strip: bool,
        /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
        ///
        /// Use as `--cargo-extra-args="--my-arg"`
        #[structopt(long = "cargo-extra-args")]
        cargo_extra_args: Vec<String>,
        /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
        ///
        /// Use as `--rustc-extra-args="--my-arg"`
        #[structopt(long = "rustc-extra-args")]
        rustc_extra_args: Vec<String>,
    },
    /// Build only a source distribution (sdist) without compiling.
    ///
    /// Building a source distribution requires a pyproject.toml with a `[build-system]` table.
    ///
    /// This command is a workaround for [pypa/pip#6041](https://github.com/pypa/pip/issues/6041)
    #[structopt(name = "sdist")]
    SDist {
        #[structopt(
            short = "m",
            long = "manifest-path",
            parse(from_os_str),
            default_value = "Cargo.toml"
        )]
        /// The path to the Cargo.toml
        manifest_path: PathBuf,
        /// The directory to store the built wheels in. Defaults to a new "wheels"
        /// directory in the project's target directory
        #[structopt(short, long, parse(from_os_str))]
        out: Option<PathBuf>,
    },
    /// Backend for the PEP 517 integration. Not for human consumption
    ///
    /// The commands are meant to be called from the python PEP 517
    #[structopt(name = "pep517")]
    PEP517(PEP517Command),
}

/// Backend for the PEP 517 integration. Not for human consumption
///
/// The commands are meant to be called from the python PEP 517
#[derive(Debug, StructOpt)]
enum PEP517Command {
    /// The implementation of prepare_metadata_for_build_wheel
    #[structopt(name = "write-dist-info")]
    WriteDistInfo {
        #[structopt(flatten)]
        build_options: BuildOptions,
        /// The metadata_directory argument to prepare_metadata_for_build_wheel
        #[structopt(long = "metadata-directory", parse(from_os_str))]
        metadata_directory: PathBuf,
        /// Strip the library for minimum file size
        #[structopt(long)]
        strip: bool,
    },
    #[structopt(name = "build-wheel")]
    /// Implementation of build_wheel
    ///
    /// --release and --strip are currently unused by the PEP 517 implementation
    BuildWheel {
        #[structopt(flatten)]
        build: BuildOptions,
        /// Strip the library for minimum file size
        #[structopt(long)]
        strip: bool,
    },
    /// The implementation of build_sdist
    #[structopt(name = "write-sdist")]
    WriteSDist {
        /// The sdist_directory argument to build_sdist
        #[structopt(long = "sdist-directory", parse(from_os_str))]
        sdist_directory: PathBuf,
        #[structopt(
            short = "m",
            long = "manifest-path",
            parse(from_os_str),
            default_value = "Cargo.toml",
            name = "PATH"
        )]
        /// The path to the Cargo.toml
        manifest_path: PathBuf,
    },
}

/// Dispatches into the native implementations of the PEP 517 functions
///
/// The last line of stdout is used as return value from the python part of the implementation
fn pep517(subcommand: PEP517Command) -> Result<(), Error> {
    match subcommand {
        PEP517Command::WriteDistInfo {
            mut build_options,
            metadata_directory,
            strip,
        } => {
            build_options.interpreter = vec!["python".to_string()];
            let context = build_options.into_build_context(true, strip)?;
            let tags = match context.bridge {
                BridgeModel::Bindings(_) => {
                    vec![context.interpreter[0].get_tag(&context.manylinux)]
                }
                BridgeModel::Bin | BridgeModel::Cffi => {
                    context.target.get_universal_tags(&context.manylinux).1
                }
            };

            let mut writer = PathWriter::from_path(metadata_directory);
            write_dist_info(&mut writer, &context.metadata21, &context.scripts, &tags)?;
            println!("{}", context.metadata21.get_dist_info_dir().display());
        }
        PEP517Command::BuildWheel { build, strip } => {
            let build_context = build.into_build_context(true, strip)?;
            let wheels = build_context.build_wheels()?;
            assert_eq!(wheels.len(), 1);
            println!("{}", wheels[0].0.file_name().unwrap().to_str().unwrap());
        }
        PEP517Command::WriteSDist {
            sdist_directory,
            manifest_path,
        } => {
            let cargo_toml = CargoToml::from_path(&manifest_path)?;
            let manifest_dir = manifest_path.parent().unwrap();
            let metadata21 = Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir)
                .context("Failed to parse Cargo.toml into python metadata")?;
            let path = source_distribution(sdist_directory, &metadata21, &manifest_path)
                .context("Failed to build source distribution")?;
            println!("{}", path.display());
        }
    };

    Ok(())
}

/// Handles authentification/keyring integration and retrying of the publish subcommand
#[cfg(feature = "upload")]
fn upload_ui(build: BuildOptions, publish: &PublishOpt, no_sdist: bool) -> Result<(), Error> {
    let build_context = build.into_build_context(!publish.debug, !publish.no_strip)?;

    if !build_context.release {
        eprintln!("⚠ Warning: You're publishing debug wheels");
    }

    let mut wheels = build_context.build_wheels()?;

    if !no_sdist {
        if let Some(source_distribution) = build_context.build_source_distribution()? {
            wheels.push(source_distribution);
        }
    }

    let (mut registry, reenter) = complete_registry(&publish)?;

    loop {
        println!("🚀 Uploading {} packages", wheels.len());

        // Upload all wheels, aborting on the first error
        let result = wheels
            .iter()
            .map(|(wheel_path, supported_versions, _)| {
                upload(
                    &registry,
                    &wheel_path,
                    &build_context.metadata21,
                    &supported_versions,
                )
            })
            .collect();

        match result {
            Ok(()) => {
                println!("✨ Packages uploaded succesfully");

                #[cfg(feature = "keyring")]
                {
                    // We know the password is correct, so we can save it in the keyring
                    let username = registry.username.clone();
                    let keyring = Keyring::new(&env!("CARGO_PKG_NAME"), &username);
                    let password = registry.password.clone();
                    keyring.set_password(&password).unwrap_or_else(|e| {
                        eprintln!("⚠ Failed to store the password in the keyring: {:?}", e)
                    });
                }

                return Ok(());
            }
            Err(UploadError::AuthenticationError) if reenter => {
                println!("⛔ Username and/or password are wrong");

                #[cfg(feature = "keyring")]
                {
                    // Delete the wrong password from the keyring
                    let old_username = registry.username.clone();
                    let keyring = Keyring::new(&env!("CARGO_PKG_NAME"), &old_username);
                    match keyring.delete_password() {
                        Ok(()) => {}
                        Err(KeyringError::NoPasswordFound) | Err(KeyringError::NoBackendFound) => {}
                        _ => eprintln!("⚠ Failed to remove password from keyring"),
                    }
                }

                let username = get_username();
                let password = rpassword::prompt_password_stdout("Please enter your password: ")
                    .unwrap_or_else(|_| {
                        // So we need this fallback for pycharm on windows
                        let mut password = String::new();
                        io::stdin()
                            .read_line(&mut password)
                            .expect("Failed to read line");
                        password.trim().to_string()
                    });

                registry = Registry::new(username, password, registry.url);
                println!("… Retrying")
            }
            Err(UploadError::AuthenticationError) => {
                bail!("Username and/or password are wrong");
            }
            Err(err) => return Err(err).context("💥 Failed to upload")?,
        }
    }
}

fn run() -> Result<(), Error> {
    #[cfg(feature = "log")]
    pretty_env_logger::init();

    let opt = Opt::from_args();

    match opt {
        Opt::Build {
            build,
            release,
            strip,
            no_sdist,
        } => {
            let build_context = build.into_build_context(release, strip)?;
            if !no_sdist {
                build_context.build_source_distribution()?;
            }
            build_context.build_wheels()?;
        }
        #[cfg(feature = "upload")]
        Opt::Publish {
            build,
            publish,
            no_sdist,
        } => {
            upload_ui(build, &publish, no_sdist)?;
        }
        Opt::ListPython => {
            let target = Target::from_target_triple(None)?;
            let found = PythonInterpreter::find_all(&target)?;
            println!("🐍 {} python interpreter found:", found.len());
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
            strip,
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
                strip,
            )?;
        }
        Opt::SDist { manifest_path, out } => {
            let manifest_dir = manifest_path.parent().unwrap();

            // Ensure the project has a compliant pyproject.toml
            get_pyproject_toml(&manifest_dir)
                .context("A pyproject.toml with a `[build-system]` table is required to build a source distribution")?;

            let cargo_toml = CargoToml::from_path(&manifest_path)?;
            let metadata21 = Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir)
                .context("Failed to parse Cargo.toml into python metadata")?;

            // Failure fails here since cargo_metadata does some weird stuff on their side
            let cargo_metadata = MetadataCommand::new()
                .manifest_path(&manifest_path)
                .exec()
                .map_err(|e| format_err!("Cargo metadata failed: {}", e))?;

            let wheel_dir = match out {
                Some(ref dir) => dir.clone(),
                None => PathBuf::from(&cargo_metadata.target_directory).join("wheels"),
            };

            fs::create_dir_all(&wheel_dir)
                .context("Failed to create the target directory for the source distribution")?;

            source_distribution(&wheel_dir, &metadata21, &manifest_path)
                .context("Failed to build source distribution")?;

        }
        Opt::PEP517(subcommand) => pep517(subcommand)?,
    }

    Ok(())
}

fn main() {
    #[cfg(feature = "human-panic")]
    {
        setup_panic!();
    }

    if let Err(e) = run() {
        eprintln!("💥 pyo3-pack failed");
        for cause in e.as_fail().iter_chain().collect::<Vec<_>>().iter().rev() {
            eprintln!("  Caused by: {}", cause);
        }
        std::process::exit(1);
    }
}
