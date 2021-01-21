//! Build and publish crates with pyo3, rust-cpython and cffi bindings as well as rust binaries
//! as python packages. This file contains the CLI and keyring integration.
//!
//! Run with --help for usage information

use anyhow::{bail, Context, Result};
#[cfg(feature = "upload")]
use bytesize::ByteSize;
use cargo_metadata::MetadataCommand;
#[cfg(feature = "upload")]
use configparser::ini::Ini;
use fs_err as fs;
#[cfg(feature = "human-panic")]
use human_panic::setup_panic;
#[cfg(feature = "password-storage")]
use keyring::{Keyring, KeyringError};
use maturin::{
    develop, get_metadata_for_distribution, get_pyproject_toml, source_distribution,
    write_dist_info, BridgeModel, BuildOptions, CargoToml, Metadata21, PathWriter,
    PythonInterpreter, Target,
};
use std::env;
use std::path::PathBuf;
use structopt::StructOpt;
#[cfg(feature = "upload")]
use {
    maturin::{upload, Registry, UploadError},
    reqwest::Url,
    rpassword,
    std::io,
};

/// Returns the password and a bool that states whether to ask for re-entering the password
/// after a failed authentication
///
/// Precedence:
/// 1. MATURIN_PASSWORD
/// 2. keyring
/// 3. stdin
#[cfg(feature = "upload")]
fn get_password(_username: &str) -> String {
    if let Ok(password) = env::var("MATURIN_PASSWORD") {
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
fn load_pypi_cred_from_config(package_name: &str) -> Option<(String, String)> {
    if let Some(mut config_path) = dirs::home_dir() {
        config_path.push(".pypirc");
        if let Ok(pypirc) = fs::read_to_string(config_path.as_path()) {
            let mut config = Ini::new();
            if config.read(pypirc).is_err() {
                return None;
            }

            if let (Some(username), Some(password)) = (
                config.get(package_name, "username"),
                config.get(package_name, "password"),
            ) {
                return Some((username, password));
            }
        }
    }

    None
}

#[cfg(feature = "upload")]
fn resolve_pypi_cred(opt: &PublishOpt, package_name: &str) -> (String, String) {
    // API token from environment variable takes priority
    if let Ok(token) = env::var("MATURIN_PYPI_TOKEN") {
        return ("__token__".to_string(), token);
    }

    // load creds from pypirc if found
    if let Some((username, password)) = load_pypi_cred_from_config(package_name) {
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

#[cfg(feature = "upload")]
/// Asks for username and password for a registry account where missing.
fn complete_registry(opt: &PublishOpt, package_name: &str) -> Result<Registry> {
    let (username, password) = resolve_pypi_cred(opt, package_name);
    let registry = Registry::new(username, password, Url::parse(&opt.registry)?);

    Ok(registry)
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
    /// through MATURIN_PASSWORD
    password: Option<String>,
    /// Do not pass --release to cargo
    #[structopt(long)]
    debug: bool,
    /// Do not strip the library for minimum file size
    #[structopt(long = "no-strip")]
    no_strip: bool,
}

#[derive(Debug, StructOpt)]
#[structopt(name = "maturin")]
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
    /// WIP Upload command, similar to twine
    ///
    /// ```
    /// maturin upload dist/*
    /// ```
    #[structopt(name = "upload", setting = structopt::clap::AppSettings::Hidden)]
    Upload {
        /// Wheels and source distributions to upload
        #[structopt(name = "FILE", parse(from_os_str))]
        files: Vec<PathBuf>,
    },
    /// Backend for the PEP 517 integration. Not for human consumption
    ///
    /// The commands are meant to be called from the python PEP 517
    #[structopt(name = "pep517", setting = structopt::clap::AppSettings::Hidden)]
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
fn pep517(subcommand: PEP517Command) -> Result<()> {
    match subcommand {
        PEP517Command::WriteDistInfo {
            build_options,
            metadata_directory,
            strip,
        } => {
            assert!(matches!(
                build_options.interpreter.as_ref(),
                Some(version) if version.len() == 1
            ));
            let context = build_options.into_build_context(true, strip)?;
            let tags = match context.bridge {
                BridgeModel::Bindings(_) => {
                    vec![context.interpreter[0].get_tag(&context.manylinux, context.universal2)]
                }
                BridgeModel::BindingsAbi3(major, minor) => {
                    let platform = context
                        .target
                        .get_platform_tag(&context.manylinux, context.universal2);
                    vec![format!("cp{}{}-abi3-{}", major, minor, platform)]
                }
                BridgeModel::Bin | BridgeModel::Cffi => {
                    context
                        .target
                        .get_universal_tags(&context.manylinux, context.universal2)
                        .1
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
            println!("{}", wheels[0].0.to_str().unwrap());
        }
        PEP517Command::WriteSDist {
            sdist_directory,
            manifest_path,
        } => {
            let cargo_toml = CargoToml::from_path(&manifest_path)?;
            let manifest_dir = manifest_path.parent().unwrap();
            let metadata21 = Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir)
                .context("Failed to parse Cargo.toml into python metadata")?;
            let path = source_distribution(sdist_directory, &metadata21, &manifest_path, None)
                .context("Failed to build source distribution")?;
            println!("{}", path.file_name().unwrap().to_str().unwrap());
        }
    };

    Ok(())
}

/// Handles authentication/keyring integration and retrying of the publish subcommand
#[cfg(feature = "upload")]
fn upload_ui(build: BuildOptions, publish: &PublishOpt, no_sdist: bool) -> Result<()> {
    let build_context = build.into_build_context(!publish.debug, !publish.no_strip)?;

    if !build_context.release {
        eprintln!("⚠  Warning: You're publishing debug wheels");
    }

    let mut wheels = build_context.build_wheels()?;

    if !no_sdist {
        if let Some(source_distribution) = build_context.build_source_distribution()? {
            wheels.push(source_distribution);
        }
    }

    let registry = complete_registry(&publish, &build_context.metadata21.name)?;

    println!("🚀 Uploading {} packages", wheels.len());

    for (wheel_path, supported_versions) in wheels {
        let upload_result = upload(
            &registry,
            &wheel_path,
            &build_context.metadata21.to_vec(),
            &supported_versions,
        );
        match upload_result {
            Ok(()) => (),
            Err(UploadError::AuthenticationError) => {
                println!("⛔ Username and/or password are wrong");

                #[cfg(feature = "keyring")]
                {
                    // Delete the wrong password from the keyring
                    let old_username = registry.username.clone();
                    let keyring = Keyring::new(&env!("CARGO_PKG_NAME"), &old_username);
                    match keyring.delete_password() {
                        Ok(()) => {
                            println!("🔑 Removed wrong password from keyring")
                        }
                        Err(KeyringError::NoPasswordFound) | Err(KeyringError::NoBackendFound) => {}
                        Err(err) => eprintln!("⚠ Failed to remove password from keyring: {}", err),
                    }
                }

                bail!("Username and/or password are wrong");
            }
            Err(err) => {
                let filesize = fs::metadata(&wheel_path)
                    .map(|x| ByteSize(x.len()).to_string())
                    .unwrap_or_else(|e| {
                        format!("Failed to get the filesize of {:?}: {}", &wheel_path, e)
                    });
                let filename = wheel_path.file_name().unwrap_or(&wheel_path.as_os_str());
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
        let keyring = Keyring::new(&env!("CARGO_PKG_NAME"), &username);
        let password = registry.password.clone();
        match keyring.set_password(&password) {
            Ok(()) => {}
            Err(KeyringError::NoBackendFound) => {}
            Err(err) => {
                eprintln!("⚠ Failed to store the password in the keyring: {:?}", err);
            }
        }
    }

    Ok(())
}

fn run() -> Result<()> {
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
            // We don't know the targeted bindings yet, so we use the most lenient
            let found = PythonInterpreter::find_all(&target, &BridgeModel::Cffi)?;
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
            let venv_dir = match (env::var_os("VIRTUAL_ENV"), env::var_os("CONDA_PREFIX")) {
                (Some(dir), None) => PathBuf::from(dir),
                (None, Some(dir)) => PathBuf::from(dir),
                (Some(_), Some(_)) => {
                    bail!("Both VIRTUAL_ENV and CONDA_PREFIX are set. Please unset one of them")
                }
                (None, None) => {
                    bail!("You need to be inside a virtualenv or conda environment to use develop (neither VIRTUAL_ENV nor CONDA_PREFIX are set)")
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
            let pyproject = get_pyproject_toml(&manifest_dir)
                .context("A pyproject.toml with a PEP 517 compliant `[build-system]` table is required to build a source distribution")?;

            let cargo_toml = CargoToml::from_path(&manifest_path)?;
            let metadata21 = Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir)
                .context("Failed to parse Cargo.toml into python metadata")?;

            let cargo_metadata = MetadataCommand::new()
                .manifest_path(&manifest_path)
                .exec()
                .context("Cargo metadata failed. Do you have cargo in your PATH?")?;

            let wheel_dir = match out {
                Some(ref dir) => dir.clone(),
                None => PathBuf::from(&cargo_metadata.target_directory).join("wheels"),
            };

            fs::create_dir_all(&wheel_dir)
                .context("Failed to create the target directory for the source distribution")?;

            source_distribution(
                &wheel_dir,
                &metadata21,
                &manifest_path,
                pyproject.sdist_include(),
            )
            .context("Failed to build source distribution")?;
        }
        Opt::PEP517(subcommand) => pep517(subcommand)?,
        Opt::Upload { files } => {
            if files.is_empty() {
                println!("⚠  Warning: No files given, exiting.");
                return Ok(());
            }
            let metadata: Vec<Vec<(String, String)>> = files
                .iter()
                .map(|path| get_metadata_for_distribution(&path))
                .collect::<Result<_>>()?;

            println!("{:?}", metadata);
        }
    }

    Ok(())
}

fn main() {
    #[cfg(feature = "human-panic")]
    {
        setup_panic!();
    }

    if let Err(e) = run() {
        eprintln!("💥 maturin failed");
        for cause in e.chain().collect::<Vec<_>>().iter() {
            eprintln!("  Caused by: {}", cause);
        }
        std::process::exit(1);
    }
}
