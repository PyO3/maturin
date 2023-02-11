//! Build and publish crates with pyo3, rust-cpython and cffi bindings as well as rust binaries
//! as python packages. This file contains the CLI and keyring integration.
//!
//! Run with --help for usage information

use anyhow::{bail, Context, Result};
#[cfg(feature = "zig")]
use cargo_zigbuild::Zig;
use clap::{CommandFactory, Parser, Subcommand};
#[cfg(feature = "scaffolding")]
use maturin::{ci::GenerateCI, init_project, new_project, GenerateProjectOptions};
use maturin::{
    develop, write_dist_info, BridgeModel, BuildOptions, CargoOptions, PathWriter, PlatformTag,
    PythonInterpreter, Target,
};
#[cfg(feature = "upload")]
use maturin::{upload_ui, PublishOpt};
use std::env;
use std::io;
use std::path::PathBuf;
use tracing::debug;

#[derive(Debug, Parser)]
#[command(
    version,
    name = env!("CARGO_PKG_NAME"),
    display_order = 1,
    after_help = "Visit https://maturin.rs to learn more about maturin."
)]
#[cfg_attr(feature = "cargo-clippy", allow(clippy::large_enum_variant))]
/// Build and publish crates with pyo3, rust-cpython and cffi bindings as well
/// as rust binaries as python packages
enum Opt {
    #[command(name = "build", alias = "b")]
    /// Build the crate into python packages
    Build {
        /// Build artifacts in release mode, with optimizations
        #[arg(short = 'r', long)]
        release: bool,
        /// Strip the library for minimum file size
        #[arg(long)]
        strip: bool,
        /// Build a source distribution
        #[arg(long)]
        sdist: bool,
        #[command(flatten)]
        build: BuildOptions,
    },
    #[cfg(feature = "upload")]
    #[command(name = "publish")]
    /// Build and publish the crate as python packages to pypi
    Publish {
        /// Do not pass --release to cargo
        #[arg(long)]
        debug: bool,
        /// Do not strip the library for minimum file size
        #[arg(long = "no-strip")]
        no_strip: bool,
        /// Don't build a source distribution
        #[arg(long = "no-sdist")]
        no_sdist: bool,
        #[command(flatten)]
        publish: PublishOpt,
        #[command(flatten)]
        build: BuildOptions,
    },
    #[command(name = "list-python")]
    /// Search and list the available python installations
    ListPython {
        #[arg(long)]
        target: Option<String>,
    },
    #[command(name = "develop", alias = "dev")]
    /// Install the crate as module in the current virtualenv
    ///
    /// Note that this command doesn't create entrypoints
    Develop {
        /// Which kind of bindings to use
        #[arg(
            short = 'b',
            long = "bindings",
            alias = "binding-crate",
            value_parser = ["pyo3", "pyo3-ffi", "rust-cpython", "cffi", "uniffi", "bin"]
        )]
        bindings: Option<String>,
        /// Pass --release to cargo
        #[arg(short = 'r', long)]
        release: bool,
        /// Strip the library for minimum file size
        #[arg(long)]
        strip: bool,
        /// Install extra requires aka. optional dependencies
        ///
        /// Use as `--extras=extra1,extra2`
        #[arg(
            short = 'E',
            long,
            value_delimiter = ',',
            action = clap::ArgAction::Append
        )]
        extras: Vec<String>,
        #[command(flatten)]
        cargo_options: CargoOptions,
    },
    /// Build only a source distribution (sdist) without compiling.
    ///
    /// Building a source distribution requires a pyproject.toml with a `[build-system]` table.
    ///
    /// This command is a workaround for [pypa/pip#6041](https://github.com/pypa/pip/issues/6041)
    #[command(name = "sdist")]
    SDist {
        #[arg(short = 'm', long = "manifest-path")]
        /// The path to the Cargo.toml
        manifest_path: Option<PathBuf>,
        /// The directory to store the built wheels in. Defaults to a new "wheels"
        /// directory in the project's target directory
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Create a new cargo project in an existing directory
    #[cfg(feature = "scaffolding")]
    #[command(name = "init")]
    InitProject {
        /// Project path
        path: Option<String>,
        #[command(flatten)]
        options: GenerateProjectOptions,
    },
    /// Create a new cargo project
    #[cfg(feature = "scaffolding")]
    #[command(name = "new")]
    NewProject {
        /// Project path
        path: String,
        #[command(flatten)]
        options: GenerateProjectOptions,
    },
    #[cfg(feature = "scaffolding")]
    #[command(name = "generate-ci")]
    GenerateCI(GenerateCI),
    /// Upload python packages to pypi
    ///
    /// It is mostly similar to `twine upload`, but can only upload python wheels
    /// and source distributions.
    #[cfg(feature = "upload")]
    #[command(name = "upload")]
    Upload {
        #[command(flatten)]
        publish: PublishOpt,
        /// The python packages to upload
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,
    },
    /// Backend for the PEP 517 integration. Not for human consumption
    ///
    /// The commands are meant to be called from the python PEP 517
    #[command(subcommand)]
    Pep517(Pep517Command),
    /// Generate shell completions
    #[command(name = "completions", hide = true)]
    Completions {
        #[arg(value_name = "SHELL")]
        shell: clap_complete_command::Shell,
    },
    /// Zig linker wrapper
    #[cfg(feature = "zig")]
    #[command(subcommand, hide = true)]
    Zig(Zig),
}

/// Backend for the PEP 517 integration. Not for human consumption
///
/// The commands are meant to be called from the python PEP 517
#[derive(Debug, Subcommand)]
#[command(name = "pep517", hide = true)]
enum Pep517Command {
    /// The implementation of prepare_metadata_for_build_wheel
    #[command(name = "write-dist-info")]
    WriteDistInfo {
        #[command(flatten)]
        build_options: BuildOptions,
        /// The metadata_directory argument to prepare_metadata_for_build_wheel
        #[arg(long = "metadata-directory")]
        metadata_directory: PathBuf,
        /// Strip the library for minimum file size
        #[arg(long)]
        strip: bool,
    },
    #[command(name = "build-wheel")]
    /// Implementation of build_wheel
    ///
    /// --release and --strip are currently unused by the PEP 517 implementation
    BuildWheel {
        #[command(flatten)]
        build_options: BuildOptions,
        /// Strip the library for minimum file size
        #[arg(long)]
        strip: bool,
        /// Build editable wheels
        #[arg(long)]
        editable: bool,
    },
    /// The implementation of build_sdist
    #[command(name = "write-sdist")]
    WriteSDist {
        /// The sdist_directory argument to build_sdist
        #[arg(long = "sdist-directory")]
        sdist_directory: PathBuf,
        #[arg(short = 'm', long = "manifest-path", value_name = "PATH")]
        /// The path to the Cargo.toml
        manifest_path: Option<PathBuf>,
    },
}

fn detect_venv(target: &Target) -> Result<PathBuf> {
    match (env::var_os("VIRTUAL_ENV"), env::var_os("CONDA_PREFIX")) {
        (Some(dir), None) => return Ok(PathBuf::from(dir)),
        (None, Some(dir)) => return Ok(PathBuf::from(dir)),
        (Some(_), Some(_)) => {
            bail!("Both VIRTUAL_ENV and CONDA_PREFIX are set. Please unset one of them")
        }
        (None, None) => {
            // No env var, try finding .venv
        }
    };

    let current_dir = env::current_dir().context("Failed to detect current directory ಠ_ಠ")?;
    // .venv in the current or any parent directory
    for dir in current_dir.ancestors() {
        let dot_venv = dir.join(".venv");
        if dot_venv.is_dir() {
            if !dot_venv.join("pyvenv.cfg").is_file() {
                bail!(
                    "Expected {} to be a virtual environment, but pyvenv.cfg is missing",
                    dot_venv.display()
                );
            }
            let python = target.get_venv_python(&dot_venv);
            if !python.is_file() {
                bail!(
                    "Your virtualenv at {} is broken. It contains a pyvenv.cfg but no python at {}",
                    dot_venv.display(),
                    python.display()
                );
            }
            debug!("Found a virtualenv named .venv at {}", dot_venv.display());
            return Ok(dot_venv);
        }
    }

    bail!(
        "Couldn't find a virtualenv or conda environment, but you need one to use this command. \
        For maturin to find your virtualenv you need to either set VIRTUAL_ENV (through activate), \
        set CONDA_PREFIX (through conda activate) or have a virtualenv called .venv in the current \
        or any parent folder. \
        See https://virtualenv.pypa.io/en/latest/index.html on how to use virtualenv or \
        use `maturin build` and `pip install <path/to/wheel>` instead."
    )
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
            assert_eq!(build_options.interpreter.len(), 1);
            let context = build_options.into_build_context(true, strip, false)?;

            // Since afaik all other PEP 517 backends also return linux tagged wheels, we do so too
            let tags = match context.bridge() {
                BridgeModel::Bindings(..) | BridgeModel::Bin(Some(..)) => {
                    vec![context.interpreter[0].get_tag(
                        &context.target,
                        &[PlatformTag::Linux],
                        context.universal2,
                    )?]
                }
                BridgeModel::BindingsAbi3(major, minor) => {
                    let platform = context
                        .target
                        .get_platform_tag(&[PlatformTag::Linux], context.universal2)?;
                    vec![format!("cp{major}{minor}-abi3-{platform}")]
                }
                BridgeModel::Bin(None) | BridgeModel::Cffi | BridgeModel::UniFfi => {
                    context
                        .target
                        .get_universal_tags(&[PlatformTag::Linux], context.universal2)?
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
                out: Some(sdist_directory),
                cargo: CargoOptions {
                    manifest_path,
                    ..Default::default()
                },
                ..Default::default()
            };
            let build_context = build_options.into_build_context(false, false, false)?;
            let (path, _) = build_context
                .build_source_distribution()?
                .context("Failed to build source distribution, pyproject.toml not found")?;
            println!("{}", path.file_name().unwrap().to_str().unwrap());
        }
    };

    Ok(())
}

fn run() -> Result<()> {
    #[cfg(feature = "log")]
    tracing_subscriber::fmt::init();

    #[cfg(feature = "zig")]
    {
        // Allow symlink `maturin` to `ar` to invoke `zig ar`
        // See https://github.com/messense/cargo-zigbuild/issues/52
        let mut args = env::args();
        let program_path = PathBuf::from(args.next().expect("no program path"));
        let program_name = program_path.file_stem().expect("no program name");
        if program_name.eq_ignore_ascii_case("ar") {
            let zig = Zig::Ar {
                args: args.collect(),
            };
            zig.execute()?;
            return Ok(());
        }
    }

    let opt = Opt::parse();

    match opt {
        Opt::Build {
            build,
            release,
            strip,
            sdist,
        } => {
            let build_context = build.into_build_context(release, strip, false)?;
            if sdist {
                build_context
                    .build_source_distribution()?
                    .context("Failed to build source distribution, pyproject.toml not found")?;
            }
            let wheels = build_context.build_wheels()?;
            assert!(!wheels.is_empty());
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
        Opt::ListPython { target } => {
            let found = if target.is_some() {
                let target = Target::from_target_triple(target)?;
                PythonInterpreter::find_by_target(&target, None)
            } else {
                let target = Target::from_target_triple(None)?;
                // We don't know the targeted bindings yet, so we use the most lenient
                PythonInterpreter::find_all(&target, &BridgeModel::Cffi, None)?
            };
            println!("🐍 {} python interpreter found:", found.len());
            for interpreter in found {
                println!(" - {interpreter}");
            }
        }
        Opt::Develop {
            bindings,
            release,
            strip,
            extras,
            cargo_options,
        } => {
            let target = Target::from_target_triple(cargo_options.target.clone())?;
            let venv_dir = detect_venv(&target)?;
            develop(bindings, cargo_options, &venv_dir, release, strip, extras)?;
        }
        Opt::SDist { manifest_path, out } => {
            let build_options = BuildOptions {
                out,
                cargo: CargoOptions {
                    manifest_path,
                    ..Default::default()
                },
                ..Default::default()
            };
            let build_context = build_options.into_build_context(false, false, false)?;
            build_context
                .build_source_distribution()?
                .context("Failed to build source distribution, pyproject.toml not found")?;
        }
        Opt::Pep517(subcommand) => pep517(subcommand)?,
        #[cfg(feature = "scaffolding")]
        Opt::InitProject { path, options } => init_project(path, options)?,
        #[cfg(feature = "scaffolding")]
        Opt::NewProject { path, options } => new_project(path, options)?,
        #[cfg(feature = "scaffolding")]
        Opt::GenerateCI(generate_ci) => generate_ci.execute()?,
        #[cfg(feature = "upload")]
        Opt::Upload { publish, files } => {
            if files.is_empty() {
                eprintln!("⚠️  Warning: No files given, exiting.");
                return Ok(());
            }

            upload_ui(&files, &publish)?
        }
        Opt::Completions { shell } => {
            shell.generate(&mut Opt::command(), &mut io::stdout());
        }
        #[cfg(feature = "zig")]
        Opt::Zig(subcommand) => {
            subcommand
                .execute()
                .context("Failed to run zig linker wrapper")?;
        }
    }

    Ok(())
}

#[cfg(not(debug_assertions))]
fn setup_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        eprintln!("\n===================================================================");
        eprintln!("maturin has panicked. This is a bug in maturin. Please report this");
        eprintln!("at https://github.com/PyO3/maturin/issues/new/choose.");
        eprintln!("If you can reliably reproduce this panic, include the");
        eprintln!("reproduction steps and re-run with the RUST_BACKTRACE=1 environment");
        eprintln!("variable set and include the backtrace in your report.");
        eprintln!();
        eprintln!("Platform: {} {}", env::consts::OS, env::consts::ARCH);
        eprintln!("Version: {}", env!("CARGO_PKG_VERSION"));
        eprintln!("Args: {}", env::args().collect::<Vec<_>>().join(" "));
        eprintln!();
        default_hook(panic_info);
        // Rust set exit code to 101 when the process panicked,
        // so here we use the same exit code
        std::process::exit(101);
    }));
}

fn main() {
    #[cfg(not(debug_assertions))]
    setup_panic_hook();

    if let Err(e) = run() {
        eprintln!("💥 maturin failed");
        for cause in e.chain() {
            eprintln!("  Caused by: {cause}");
        }
        std::process::exit(1);
    }
}
