//! Build and publish crates with pyo3, rust-cpython and cffi bindings as well as rust binaries
//! as python packages. This file contains the CLI and keyring integration.
//!
//! Run with --help for usage information

use anyhow::{bail, Context, Result};
use maturin::{
    develop, init_project, new_project, write_dist_info, BridgeModel, BuildOptions,
    GenerateProjectOptions, PathWriter, PlatformTag, PythonInterpreter, Target,
};
#[cfg(feature = "upload")]
use maturin::{upload_ui, PublishOpt};
use std::env;
use std::io;
use std::path::PathBuf;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(name = env!("CARGO_PKG_NAME"))]
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
        /// Do not pass --release to cargo
        #[structopt(long)]
        debug: bool,
        /// Do not strip the library for minimum file size
        #[structopt(long = "no-strip")]
        no_strip: bool,
        /// Don't build a source distribution
        #[structopt(long = "no-sdist")]
        no_sdist: bool,
        #[structopt(flatten)]
        publish: PublishOpt,
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
        /// Install extra requires aka. optional dependencies
        ///
        /// Use as `--extras=extra1,extra2`
        #[structopt(short = "E", long, use_delimiter = true, multiple = false)]
        extras: Vec<String>,
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
    /// Create a new cargo project in an existing directory
    #[structopt(name = "init")]
    InitProject {
        /// Project path
        path: Option<String>,
        #[structopt(flatten)]
        options: GenerateProjectOptions,
    },
    /// Create a new cargo project
    #[structopt(name = "new")]
    NewProject {
        /// Project path
        path: String,
        #[structopt(flatten)]
        options: GenerateProjectOptions,
    },
    /// Uploads python packages to pypi
    ///
    /// It is mostly similar to `twine upload`, but can only upload python wheels
    /// and source distributions.
    #[cfg(feature = "upload")]
    #[structopt(name = "upload")]
    Upload {
        #[structopt(flatten)]
        publish: PublishOpt,
        /// The python packages to upload
        #[structopt(name = "FILE", parse(from_os_str))]
        files: Vec<PathBuf>,
    },
    /// Backend for the PEP 517 integration. Not for human consumption
    ///
    /// The commands are meant to be called from the python PEP 517
    #[structopt(name = "pep517", setting = structopt::clap::AppSettings::Hidden)]
    Pep517(Pep517Command),
    /// Generate shell completions
    #[structopt(name = "completions", setting = structopt::clap::AppSettings::Hidden)]
    Completions {
        #[structopt(name = "SHELL", parse(try_from_str))]
        shell: structopt::clap::Shell,
    },
}

/// Backend for the PEP 517 integration. Not for human consumption
///
/// The commands are meant to be called from the python PEP 517
#[derive(Debug, StructOpt)]
enum Pep517Command {
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
        build_options: BuildOptions,
        /// Strip the library for minimum file size
        #[structopt(long)]
        strip: bool,
        /// Build editable wheels
        #[structopt(long)]
        editable: bool,
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
fn pep517(subcommand: Pep517Command) -> Result<()> {
    match subcommand {
        Pep517Command::WriteDistInfo {
            build_options,
            metadata_directory,
            strip,
        } => {
            assert!(matches!(
                build_options.interpreter.as_ref(),
                Some(version) if version.len() == 1
            ));
            let context = build_options.into_build_context(true, strip, false)?;

            // Since afaik all other PEP 517 backends also return linux tagged wheels, we do so too
            let tags = match context.bridge {
                BridgeModel::Bindings(_) => {
                    vec![context.interpreter[0].get_tag(PlatformTag::Linux, context.universal2)?]
                }
                BridgeModel::BindingsAbi3(major, minor) => {
                    let platform = context
                        .target
                        .get_platform_tag(PlatformTag::Linux, context.universal2)?;
                    vec![format!("cp{}{}-abi3-{}", major, minor, platform)]
                }
                BridgeModel::Bin | BridgeModel::Cffi => {
                    context
                        .target
                        .get_universal_tags(PlatformTag::Linux, context.universal2)?
                        .1
                }
            };

            let mut writer = PathWriter::from_path(metadata_directory);
            write_dist_info(&mut writer, &context.metadata21, &tags)?;
            println!("{}", context.metadata21.get_dist_info_dir().display());
        }
        Pep517Command::BuildWheel {
            build_options,
            strip,
            editable,
        } => {
            let build_context = build_options.into_build_context(true, strip, editable)?;
            let wheels = build_context.build_wheels()?;
            assert_eq!(wheels.len(), 1);
            println!("{}", wheels[0].0.to_str().unwrap());
        }
        Pep517Command::WriteSDist {
            sdist_directory,
            manifest_path,
        } => {
            let build_options = BuildOptions {
                manifest_path,
                out: Some(sdist_directory),
                ..Default::default()
            };
            let build_context = build_options.into_build_context(false, false, false)?;
            let (path, _) = build_context
                .build_source_distribution()?
                .context("Failed to build source distribution")?;
            println!("{}", path.file_name().unwrap().to_str().unwrap());
        }
    };

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
            let build_context = build.into_build_context(release, strip, false)?;
            if !no_sdist {
                build_context.build_source_distribution()?;
            }
            build_context.build_wheels()?;
        }
        #[cfg(feature = "upload")]
        Opt::Publish {
            build,
            publish,
            debug,
            no_strip,
            no_sdist,
        } => {
            let build_context = build.into_build_context(!debug, !no_strip, false)?;

            if !build_context.release {
                eprintln!("⚠️  Warning: You're publishing debug wheels");
            }

            let mut wheels = build_context.build_wheels()?;
            if !no_sdist {
                if let Some(sd) = build_context.build_source_distribution()? {
                    wheels.push(sd);
                }
            }

            let items = wheels.into_iter().map(|wheel| wheel.0).collect::<Vec<_>>();

            upload_ui(&items, &publish)?
        }
        Opt::ListPython => {
            let target = Target::from_target_triple(None)?;
            // We don't know the targeted bindings yet, so we use the most lenient
            let found = PythonInterpreter::find_all(&target, &BridgeModel::Cffi, None)?;
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
            extras,
        } => {
            let venv_dir = match (env::var_os("VIRTUAL_ENV"), env::var_os("CONDA_PREFIX")) {
                (Some(dir), None) => PathBuf::from(dir),
                (None, Some(dir)) => PathBuf::from(dir),
                (Some(_), Some(_)) => {
                    bail!("Both VIRTUAL_ENV and CONDA_PREFIX are set. Please unset one of them")
                }
                (None, None) => {
                    bail!(
                        "You need to be inside a virtualenv or conda environment to use develop \
                        (neither VIRTUAL_ENV nor CONDA_PREFIX are set). \
                        See https://virtualenv.pypa.io/en/latest/index.html on how to use virtualenv or \
                        use `maturin build` and `pip install <path/to/wheel>` instead."
                    )
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
                extras,
            )?;
        }
        Opt::SDist { manifest_path, out } => {
            let build_options = BuildOptions {
                manifest_path,
                out,
                ..Default::default()
            };
            let build_context = build_options.into_build_context(false, false, false)?;
            build_context
                .build_source_distribution()?
                .context("Failed to build source distribution")?;
        }
        Opt::Pep517(subcommand) => pep517(subcommand)?,
        Opt::InitProject { path, options } => init_project(path, options)?,
        Opt::NewProject { path, options } => new_project(path, options)?,
        #[cfg(feature = "upload")]
        Opt::Upload { publish, files } => {
            if files.is_empty() {
                println!("⚠️  Warning: No files given, exiting.");
                return Ok(());
            }

            upload_ui(&files, &publish)?
        }
        Opt::Completions { shell } => {
            Opt::clap().gen_completions_to(env!("CARGO_BIN_NAME"), shell, &mut io::stdout());
        }
    }

    Ok(())
}

fn main() {
    #[cfg(feature = "human-panic")]
    {
        human_panic::setup_panic!();
    }

    if let Err(e) = run() {
        eprintln!("💥 maturin failed");
        for cause in e.chain().collect::<Vec<_>>().iter() {
            eprintln!("  Caused by: {}", cause);
        }
        std::process::exit(1);
    }
}
