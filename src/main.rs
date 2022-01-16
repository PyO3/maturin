//! Build and publish crates with pyo3, rust-cpython and cffi bindings as well as rust binaries
//! as python packages. This file contains the CLI and keyring integration.
//!
//! Run with --help for usage information

use anyhow::{bail, Context, Result};
use clap::{ArgEnum, IntoApp, Parser};
use clap_complete::Generator;
use maturin::{
    develop, init_project, new_project, write_dist_info, BridgeModel, BuildOptions,
    GenerateProjectOptions, PathWriter, PlatformTag, PythonInterpreter, Target, Zig,
};
#[cfg(feature = "upload")]
use maturin::{upload_ui, PublishOpt};
use std::env;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Parser)]
#[clap(name = env!("CARGO_PKG_NAME"))]
#[cfg_attr(feature = "cargo-clippy", allow(clippy::large_enum_variant))]
/// Build and publish crates with pyo3, rust-cpython and cffi bindings as well
/// as rust binaries as python packages
enum Opt {
    #[clap(name = "build")]
    /// Build the crate into python packages
    Build {
        #[clap(flatten)]
        build: BuildOptions,
        /// Pass --release to cargo
        #[clap(long)]
        release: bool,
        /// Strip the library for minimum file size
        #[clap(long)]
        strip: bool,
        /// Don't build a source distribution
        #[clap(long = "no-sdist")]
        no_sdist: bool,
    },
    #[cfg(feature = "upload")]
    #[clap(name = "publish")]
    /// Build and publish the crate as python packages to pypi
    Publish {
        #[clap(flatten)]
        build: BuildOptions,
        /// Do not pass --release to cargo
        #[clap(long)]
        debug: bool,
        /// Do not strip the library for minimum file size
        #[clap(long = "no-strip")]
        no_strip: bool,
        /// Don't build a source distribution
        #[clap(long = "no-sdist")]
        no_sdist: bool,
        #[clap(flatten)]
        publish: PublishOpt,
    },
    #[clap(name = "list-python")]
    /// Searches and lists the available python installations
    ListPython,
    #[clap(name = "develop")]
    /// Installs the crate as module in the current virtualenv
    ///
    /// Note that this command doesn't create entrypoints
    Develop {
        /// Which kind of bindings to use. Possible values are pyo3, rust-cpython, cffi and bin
        #[clap(short = 'b', long = "bindings", alias = "binding-crate")]
        bindings: Option<String>,
        #[clap(
            short = 'm',
            long = "manifest-path",
            parse(from_os_str),
            default_value = "Cargo.toml"
        )]
        /// The path to the Cargo.toml
        manifest_path: PathBuf,
        /// Pass --release to cargo
        #[clap(long)]
        release: bool,
        /// Strip the library for minimum file size
        #[clap(long)]
        strip: bool,
        /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
        ///
        /// Use as `--cargo-extra-args="--my-arg"`
        #[clap(long = "cargo-extra-args")]
        cargo_extra_args: Vec<String>,
        /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
        ///
        /// Use as `--rustc-extra-args="--my-arg"`
        #[clap(long = "rustc-extra-args")]
        rustc_extra_args: Vec<String>,
        /// Install extra requires aka. optional dependencies
        ///
        /// Use as `--extras=extra1,extra2`
        #[clap(
            short = 'E',
            long,
            use_delimiter = true,
            multiple_values = false,
            multiple_occurrences = false
        )]
        extras: Vec<String>,
        /// The `--profile` to pass to `cargo`.
        #[clap(long)]
        profile: Option<String>,
    },
    /// Build only a source distribution (sdist) without compiling.
    ///
    /// Building a source distribution requires a pyproject.toml with a `[build-system]` table.
    ///
    /// This command is a workaround for [pypa/pip#6041](https://github.com/pypa/pip/issues/6041)
    #[clap(name = "sdist")]
    SDist {
        #[clap(
            short = 'm',
            long = "manifest-path",
            parse(from_os_str),
            default_value = "Cargo.toml"
        )]
        /// The path to the Cargo.toml
        manifest_path: PathBuf,
        /// The directory to store the built wheels in. Defaults to a new "wheels"
        /// directory in the project's target directory
        #[clap(short, long, parse(from_os_str))]
        out: Option<PathBuf>,
    },
    /// Create a new cargo project in an existing directory
    #[clap(name = "init")]
    InitProject {
        /// Project path
        path: Option<String>,
        #[clap(flatten)]
        options: GenerateProjectOptions,
    },
    /// Create a new cargo project
    #[clap(name = "new")]
    NewProject {
        /// Project path
        path: String,
        #[clap(flatten)]
        options: GenerateProjectOptions,
    },
    /// Uploads python packages to pypi
    ///
    /// It is mostly similar to `twine upload`, but can only upload python wheels
    /// and source distributions.
    #[cfg(feature = "upload")]
    #[clap(name = "upload")]
    Upload {
        #[clap(flatten)]
        publish: PublishOpt,
        /// The python packages to upload
        #[clap(name = "FILE", parse(from_os_str))]
        files: Vec<PathBuf>,
    },
    /// Backend for the PEP 517 integration. Not for human consumption
    ///
    /// The commands are meant to be called from the python PEP 517
    #[clap(subcommand)]
    Pep517(Pep517Command),
    /// Generate shell completions
    #[clap(name = "completions", setting = clap::AppSettings::Hidden)]
    Completions {
        #[clap(name = "SHELL", parse(try_from_str))]
        shell: Shell,
    },
    /// Zig linker wrapper
    #[clap(subcommand)]
    Zig(Zig),
}

/// Backend for the PEP 517 integration. Not for human consumption
///
/// The commands are meant to be called from the python PEP 517
#[derive(Debug, Parser)]
#[clap(name = "pep517", setting = clap::AppSettings::Hidden)]
enum Pep517Command {
    /// The implementation of prepare_metadata_for_build_wheel
    #[clap(name = "write-dist-info")]
    WriteDistInfo {
        #[clap(flatten)]
        build_options: BuildOptions,
        /// The metadata_directory argument to prepare_metadata_for_build_wheel
        #[clap(long = "metadata-directory", parse(from_os_str))]
        metadata_directory: PathBuf,
        /// Strip the library for minimum file size
        #[clap(long)]
        strip: bool,
    },
    #[clap(name = "build-wheel")]
    /// Implementation of build_wheel
    ///
    /// --release and --strip are currently unused by the PEP 517 implementation
    BuildWheel {
        #[clap(flatten)]
        build_options: BuildOptions,
        /// Strip the library for minimum file size
        #[clap(long)]
        strip: bool,
        /// Build editable wheels
        #[clap(long)]
        editable: bool,
    },
    /// The implementation of build_sdist
    #[clap(name = "write-sdist")]
    WriteSDist {
        /// The sdist_directory argument to build_sdist
        #[clap(long = "sdist-directory", parse(from_os_str))]
        sdist_directory: PathBuf,
        #[clap(
            short = 'm',
            long = "manifest-path",
            parse(from_os_str),
            default_value = "Cargo.toml",
            name = "PATH"
        )]
        /// The path to the Cargo.toml
        manifest_path: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, ArgEnum)]
#[allow(clippy::enum_variant_names)]
enum Shell {
    Bash,
    Elvish,
    Fish,
    PowerShell,
    Zsh,
    Fig,
}

impl FromStr for Shell {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "bash" => Ok(Shell::Bash),
            "elvish" => Ok(Shell::Elvish),
            "fish" => Ok(Shell::Fish),
            "powershell" => Ok(Shell::PowerShell),
            "zsh" => Ok(Shell::Zsh),
            "fig" => Ok(Shell::Fig),
            _ => Err("[valid values: bash, elvish, fish, powershell, zsh, fig]".to_string()),
        }
    }
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
            let context = build_options.into_build_context(Some("release"), strip, false)?;

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
            let build_context = build_options.into_build_context(Some("release"), strip, editable)?;
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
            let build_context = build_options.into_build_context(None, false, false)?;
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

    let opt = Opt::parse();

    match opt {
        Opt::Build {
            mut build,
            release,
            strip,
            no_sdist,
        } => {
            build.profile = match (release, build.profile) {
                (true, Some(_)) => bail!("cannot set both `--release` and `--profile`"),
                (true, None) => Some("release".to_owned()),
                (false, profile) => profile
            };
            let build_context = build.into_build_context(None, strip, false)?;
            if !no_sdist {
                build_context.build_source_distribution()?;
            }
            build_context.build_wheels()?;
        }
        #[cfg(feature = "upload")]
        Opt::Publish {
            mut build,
            publish,
            debug,
            no_strip,
            no_sdist,
        } => {
            build.profile = match (debug, build.profile) {
                (true, Some(_)) => bail!("cannot set both `--debug` and `--profile`"),
                (true, None) => Some("debug".to_owned()),
                (false, profile) => profile,
            };
            let build_context = build.into_build_context(Some("release"), !no_strip, false)?;

            if build_context.profile.as_deref() != Some("release") {
                eprintln!("⚠️  Warning: You're not publishing release wheels");
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
            bindings,
            manifest_path,
            cargo_extra_args,
            rustc_extra_args,
            release,
            strip,
            extras,
            profile,
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
            let profile = match (release, profile) {
                (true, Some(_)) => bail!("cannot set both `--release` and `--profile`"),
                (true, None) => Some("release".to_owned()),
                (false, profile) => profile
            };

            develop(
                bindings,
                &manifest_path,
                cargo_extra_args,
                rustc_extra_args,
                &venv_dir,
                strip,
                extras,
                profile,
            )?;
        }
        Opt::SDist { manifest_path, out } => {
            let build_options = BuildOptions {
                manifest_path,
                out,
                ..Default::default()
            };
            let build_context = build_options.into_build_context(None, false, false)?;
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
            let mut app = Opt::into_app();
            match shell {
                Shell::Fig => {
                    app.set_bin_name(env!("CARGO_BIN_NAME"));
                    let fig = clap_complete_fig::Fig;
                    fig.generate(&app, &mut io::stdout());
                }
                _ => {
                    let shell = match shell {
                        Shell::Bash => clap_complete::Shell::Bash,
                        Shell::Elvish => clap_complete::Shell::Elvish,
                        Shell::Fish => clap_complete::Shell::Fish,
                        Shell::PowerShell => clap_complete::Shell::PowerShell,
                        Shell::Zsh => clap_complete::Shell::Zsh,
                        Shell::Fig => unreachable!(),
                    };
                    clap_complete::generate(
                        shell,
                        &mut app,
                        env!("CARGO_BIN_NAME"),
                        &mut io::stdout(),
                    )
                }
            }
        }
        Opt::Zig(subcommand) => {
            subcommand
                .execute()
                .context("Failed to create zig wrapper script")?;
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
