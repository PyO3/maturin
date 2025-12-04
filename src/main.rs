//! Build and publish crates with pyo3, cffi and uniffi bindings as well as rust binaries
//! as python packages. This file contains the CLI and keyring integration.
//!
//! Run with --help for usage information

use anyhow::{Context, Result, bail};
use cargo_options::heading;
#[cfg(feature = "zig")]
use cargo_zigbuild::Zig;
#[cfg(feature = "cli-completion")]
use clap::CommandFactory;
use clap::{Parser, Subcommand};
use ignore::overrides::Override;
use maturin::{
    BridgeModel, BuildOptions, CargoOptions, DevelopOptions, PathWriter, PythonInterpreter, Target,
    TargetTriple, VirtualWriter, develop, find_path_deps, write_dist_info,
};
#[cfg(feature = "schemars")]
use maturin::{GenerateJsonSchemaOptions, generate_json_schema};
#[cfg(feature = "scaffolding")]
use maturin::{GenerateProjectOptions, ci::GenerateCI, init_project, new_project};
#[cfg(feature = "upload")]
use maturin::{PublishOpt, upload_ui};
use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{debug, instrument};
use tracing_subscriber::filter::Directive;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, Parser)]
#[command(
    version,
    name = env!("CARGO_PKG_NAME"),
    display_order = 1,
    after_help = "Visit https://maturin.rs to learn more about maturin.",
    styles = cargo_options::styles(),
)]
/// Build and publish crates with pyo3, cffi and uniffi bindings as well
/// as rust binaries as python packages
struct Opt {
    /// Use verbose output.
    ///
    /// * Default: Show build information and `cargo build` output.
    /// * `-v`: Use `cargo build -v`.
    /// * `-vv`: Show debug logging and use `cargo build -vv`.
    /// * `-vvv`: Show trace logging.
    ///
    /// You can configure fine-grained logging using the `RUST_LOG` environment variable.
    /// (<https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives>)
    #[arg(global = true, action = clap::ArgAction::Count, long, short)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Parser)]
#[allow(clippy::large_enum_variant)]
/// Build and publish crates with pyo3, cffi and uniffi bindings as well
/// as rust binaries as python packages
enum Command {
    #[command(name = "build", alias = "b")]
    /// Build the crate into python packages
    Build {
        /// Build artifacts in release mode, with optimizations
        #[arg(short = 'r', long, help_heading = heading::COMPILATION_OPTIONS, conflicts_with = "profile")]
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
        #[arg(long, conflicts_with = "profile")]
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
        target: Option<TargetTriple>,
    },
    #[command(name = "develop", alias = "dev")]
    /// Install the crate as module in the current virtualenv
    Develop(DevelopOptions),
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
    #[cfg(feature = "cli-completion")]
    #[command(name = "completions", hide = true)]
    Completions {
        #[arg(value_name = "SHELL")]
        shell: clap_complete_command::Shell,
    },
    /// Zig linker wrapper
    #[cfg(feature = "zig")]
    #[command(subcommand, hide = true)]
    Zig(Zig),
    /// Generate the JSON schema for the `pyproject.toml` file.
    #[cfg(feature = "schemars")]
    #[command(name = "generate-json-schema", hide = true)]
    GenerateJsonSchema(GenerateJsonSchemaOptions),
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
        (Some(venv), Some(conda)) if venv == conda => return Ok(PathBuf::from(venv)),
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
            let mut context = build_options
                .into_build_context()
                .strip(strip)
                .editable(false)
                .build()?;

            // TBD: does `--profile release` do anything here?
            if context.cargo_options.profile.is_none() {
                context.cargo_options.profile = Some("release".to_string());
            }

            let mut writer =
                VirtualWriter::new(PathWriter::from_path(metadata_directory), Override::empty());
            let dist_info_dir = write_dist_info(
                &mut writer,
                &context.project_layout.project_root,
                &context.metadata24,
                &context.tags_from_bridge()?,
            )?;
            writer.finish()?;
            println!("{}", dist_info_dir.display());
        }
        Pep517Command::BuildWheel {
            build_options,
            strip,
            editable,
        } => {
            let mut build_context = build_options
                .into_build_context()
                .strip(strip)
                .editable(editable)
                .build()?;
            if build_context.cargo_options.profile.is_none() {
                build_context.cargo_options.profile = Some("release".to_string());
            }
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
                    // Enable all features to ensure all optional path dependencies are packaged
                    // into source distribution
                    all_features: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            let build_context = build_options
                .into_build_context()
                .strip(false)
                .editable(false)
                .sdist_only(true)
                .build()?;
            let (path, _) = build_context
                .build_source_distribution()?
                .context("Failed to build source distribution, pyproject.toml not found")?;
            println!("{}", path.file_name().unwrap().to_str().unwrap());
        }
    };

    Ok(())
}

#[instrument]
fn run() -> Result<()> {
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

    #[cfg(not(feature = "wild"))]
    let opt = Opt::parse();
    #[cfg(feature = "wild")]
    let opt = Opt::parse_from(wild::args_os());

    setup_logging(opt.verbose)?;

    match opt.command {
        Command::Build {
            mut build,
            release,
            strip,
            sdist,
        } => {
            // set profile to release if specified; `--release` and `--profile` are mutually exclusive
            if release {
                build.profile = Some("release".to_string());
            }
            let build_context = build
                .into_build_context()
                .strip(strip)
                .editable(false)
                .build()?;
            if sdist {
                build_context
                    .build_source_distribution()?
                    .context("Failed to build source distribution, pyproject.toml not found")?;
            }
            let wheels = build_context.build_wheels()?;
            assert!(!wheels.is_empty());
        }
        #[cfg(feature = "upload")]
        Command::Publish {
            mut build,
            mut publish,
            debug,
            no_strip,
            no_sdist,
        } => {
            // set profile to dev if specified; `--debug` and `--profile` are mutually exclusive
            //
            // do it here to take precedence over pyproject.toml profile setting
            if debug {
                build.profile = Some("dev".to_string());
            }

            let mut build_context = build
                .into_build_context()
                .strip(!no_strip)
                .editable(false)
                .build()?;

            // ensure profile always set when publishing
            // (respect pyproject.toml if set)
            // don't need to check `debug` here, set above to take precedence if set
            let profile = build_context
                .cargo_options
                .profile
                .get_or_insert_with(|| "release".to_string());

            if profile == "dev" {
                eprintln!("⚠️  Warning: You're publishing debug wheels");
            }

            let mut wheels = build_context.build_wheels()?;
            if !no_sdist {
                if let Some(sd) = build_context.build_source_distribution()? {
                    wheels.push(sd);
                }
            }

            let items = wheels.into_iter().map(|wheel| wheel.0).collect::<Vec<_>>();
            publish.non_interactive_on_ci();

            upload_ui(&items, &publish)?
        }
        Command::ListPython { target } => {
            let found = if target.is_some() {
                let target = Target::from_target_triple(target.as_ref())?;
                PythonInterpreter::find_by_target(&target, None, None)
            } else {
                let target = Target::from_target_triple(None)?;
                // We don't know the targeted bindings yet, so we use the most lenient
                PythonInterpreter::find_all(&target, &BridgeModel::Cffi, None)?
            };
            eprintln!("🐍 {} python interpreter found:", found.len());
            for interpreter in found {
                eprintln!(" - {interpreter}");
            }
        }
        Command::Develop(develop_options) => {
            let target = Target::from_target_triple(develop_options.cargo_options.target.as_ref())?;
            let venv_dir = detect_venv(&target)?;
            develop(develop_options, &venv_dir)?;
        }
        Command::SDist { manifest_path, out } => {
            // Get cargo metadata to check for path dependencies
            let cargo_metadata_result = cargo_metadata::MetadataCommand::new()
                .cargo_path("cargo")
                .manifest_path(
                    manifest_path
                        .as_deref()
                        .unwrap_or_else(|| std::path::Path::new("Cargo.toml")),
                )
                .verbose(true)
                .exec();

            let has_path_deps = cargo_metadata_result
                .ok()
                .and_then(|metadata| find_path_deps(&metadata).ok())
                .map(|path_deps| !path_deps.is_empty())
                .unwrap_or(false); // If we can't get metadata, don't force all features
            let build_options = BuildOptions {
                out,
                cargo: CargoOptions {
                    manifest_path,
                    // Only enable all features when we have local path dependencies
                    // to ensure they are packaged into source distribution
                    all_features: has_path_deps,
                    ..Default::default()
                },
                ..Default::default()
            };
            let build_context = build_options
                .into_build_context()
                .strip(false)
                .editable(false)
                .sdist_only(true)
                .build()?;
            build_context
                .build_source_distribution()?
                .context("Failed to build source distribution, pyproject.toml not found")?;
        }
        Command::Pep517(subcommand) => pep517(subcommand)?,
        #[cfg(feature = "scaffolding")]
        Command::InitProject { path, options } => init_project(path, options)?,
        #[cfg(feature = "scaffolding")]
        Command::NewProject { path, options } => new_project(path, options)?,
        #[cfg(feature = "scaffolding")]
        Command::GenerateCI(generate_ci) => generate_ci.execute()?,
        #[cfg(feature = "upload")]
        Command::Upload { mut publish, files } => {
            if files.is_empty() {
                eprintln!("⚠️  Warning: No files given, exiting.");
                return Ok(());
            }
            publish.non_interactive_on_ci();

            upload_ui(&files, &publish)?
        }
        #[cfg(feature = "cli-completion")]
        Command::Completions { shell } => {
            shell.generate(&mut Opt::command(), &mut std::io::stdout());
        }
        #[cfg(feature = "zig")]
        Command::Zig(subcommand) => {
            subcommand
                .execute()
                .context("Failed to run zig linker wrapper")?;
        }
        #[cfg(feature = "schemars")]
        Command::GenerateJsonSchema(args) => generate_json_schema(args)?,
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

fn setup_logging(verbose: u8) -> Result<()> {
    // `RUST_LOG` takes precedence over these
    let default_directive = match verbose {
        // `-v` runs `cargo build -v`, but doesn't show maturin debug logging yet.
        0..=1 => tracing::level_filters::LevelFilter::OFF.into(),
        2 => Directive::from_str("debug").unwrap(),
        3.. => Directive::from_str("trace").unwrap(),
    };

    let filter = EnvFilter::builder()
        .with_default_directive(default_directive)
        .from_env()
        .context("Invalid RUST_LOG directives")?;

    let logger = tracing_subscriber::fmt::layer()
        // Avoid showing all the details from the spans
        .compact()
        // Log the timing of each span
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE);

    tracing_subscriber::registry()
        .with(logger.with_filter(filter))
        .init();

    Ok(())
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
