//! Build and publish crates with pyo3, cffi and uniffi bindings as well as rust binaries
//! as python packages. This file contains the CLI and keyring integration.
//!
//! Run with --help for usage information

use anyhow::{Context, Result};
use cargo_options::heading;
#[cfg(feature = "zig")]
use cargo_zigbuild::Zig;
#[cfg(feature = "cli-completion")]
use clap::CommandFactory;
use clap::Parser;
#[cfg(feature = "schemars")]
use maturin::GenerateJsonSchemaOptions;
#[cfg(feature = "upload")]
use maturin::PublishOpt;
use maturin::{BuildOptions, CargoOptions, DevelopOptions, PythonOptions, TargetTriple};
#[cfg(feature = "scaffolding")]
use maturin::{GenerateProjectOptions, ci::GenerateCI};
use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::instrument;
use tracing_subscriber::filter::Directive;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

mod commands;
use crate::commands::StripOption;
use crate::commands::pep517::Pep517Command;

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
/// Build and publish crates with pyo3, cffi and uniffi bindings as well
/// as rust binaries as python packages
enum Command {
    #[command(name = "build", alias = "b")]
    /// Build the crate into python packages
    Build {
        /// Build artifacts in release mode, with optimizations
        #[arg(short = 'r', long, help_heading = heading::COMPILATION_OPTIONS, conflicts_with = "profile")]
        release: bool,
        #[command(flatten)]
        strip_opt: StripOption,
        /// Build a source distribution and build wheels from it.
        ///
        /// This verifies that the source distribution is complete and can be
        /// used to build the project from source.
        #[arg(long)]
        sdist: bool,
        /// Build with Profile-Guided Optimization (PGO).
        ///
        /// Requires `pgo-command` to be set in `[tool.maturin]` in pyproject.toml.
        /// This performs a three-phase build: instrumented build, profile training,
        /// and optimized rebuild.
        #[arg(long)]
        pgo: bool,
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
        /// Build with Profile-Guided Optimization (PGO).
        ///
        /// Requires `pgo-command` to be set in `[tool.maturin]` in pyproject.toml.
        #[arg(long)]
        pgo: bool,
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
    /// Autogenerate type stubs
    #[command(name = "generate-stubs")]
    GenerateStub {
        /// The directory to store the type stubs in
        #[arg(short, long)]
        out: PathBuf,
        /// Python and bindings options
        #[command(flatten)]
        python: PythonOptions,
        /// Cargo build options
        #[command(flatten)]
        cargo: CargoOptions,
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

#[instrument]
fn run() -> Result<()> {
    #[cfg(feature = "zig")]
    {
        // Allow symlink `maturin` to various tool names to invoke zig wrappers
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
        } else if program_name.eq_ignore_ascii_case("lib") {
            let zig = Zig::Lib {
                args: args.collect(),
            };
            zig.execute()?;
            return Ok(());
        } else if program_name.to_string_lossy().ends_with("dlltool") {
            let zig = Zig::Dlltool {
                args: args.collect(),
            };
            zig.execute()?;
            return Ok(());
        } else if program_name.eq_ignore_ascii_case("install_name_tool") {
            cargo_zigbuild::macos::install_name_tool::execute(args)?;
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
            build,
            release,
            strip_opt,
            sdist,
            pgo,
        } => commands::build::build(build, release, strip_opt, sdist, pgo)?,
        #[cfg(feature = "upload")]
        Command::Publish {
            build,
            publish,
            debug,
            no_strip,
            no_sdist,
            pgo,
        } => commands::publish::publish(build, publish, debug, no_strip, no_sdist, pgo)?,
        Command::ListPython { target } => commands::list_python(target)?,
        Command::Develop(develop_options) => commands::develop::develop_cmd(develop_options)?,
        Command::SDist { manifest_path, out } => commands::sdist::sdist(manifest_path, out)?,
        Command::Pep517(subcommand) => commands::pep517::pep517(subcommand)?,
        #[cfg(feature = "scaffolding")]
        Command::InitProject { path, options } => commands::init_project(path, options)?,
        #[cfg(feature = "scaffolding")]
        Command::NewProject { path, options } => commands::new_project(path, options)?,
        #[cfg(feature = "scaffolding")]
        Command::GenerateCI(generate_ci) => commands::generate_ci(generate_ci)?,
        #[cfg(feature = "upload")]
        Command::Upload { publish, files } => commands::upload(publish, files)?,
        Command::GenerateStub { out, python, cargo } => {
            commands::generate_stubs::generate_stubs(out, python, cargo)?
        }
        #[cfg(feature = "cli-completion")]
        Command::Completions { shell } => {
            commands::completions(shell, &mut Opt::command());
        }
        #[cfg(feature = "zig")]
        Command::Zig(subcommand) => commands::zig(subcommand)?,
        #[cfg(feature = "schemars")]
        Command::GenerateJsonSchema(args) => commands::generate_json_schema(args)?,
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
