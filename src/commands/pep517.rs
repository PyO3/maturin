use anyhow::{Context, Result, bail};
use clap::Subcommand;
use ignore::overrides::Override;
use maturin::{
    BuildContext, BuildOptions, BuildOrchestrator, CargoOptions, OutputOptions, PathWriter,
    VirtualWriter, write_dist_info,
};
use std::path::PathBuf;
use tracing::instrument;

use crate::commands::StripOption;

/// Backend for the PEP 517 integration. Not for human consumption
///
/// The commands are meant to be called from the python PEP 517
#[derive(Debug, Subcommand)]
#[command(name = "pep517", hide = true)]
pub enum Pep517Command {
    /// The implementation of prepare_metadata_for_build_wheel
    #[command(name = "write-dist-info")]
    WriteDistInfo {
        #[command(flatten)]
        build_options: BuildOptions,
        /// The metadata_directory argument to prepare_metadata_for_build_wheel
        #[arg(long = "metadata-directory")]
        metadata_directory: PathBuf,
        #[command(flatten)]
        strip_opt: StripOption,
    },
    #[command(name = "build-wheel")]
    /// Implementation of build_wheel
    BuildWheel {
        #[command(flatten)]
        build_options: BuildOptions,
        #[command(flatten)]
        strip_opt: StripOption,
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

/// Ensure a PEP 517 hook is scoped to the single interpreter running pip.
///
/// Duplicate interpreter paths can be supplied when user-set
/// `MATURIN_PEP517_ARGS=--interpreter ...` combines with the auto-injected
/// interpreter, so collapse duplicates first. Only genuinely different
/// interpreters should error here, before later hook code can panic.
fn require_single_interpreter(opts: &mut BuildOptions, cmd: &str) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    opts.python.interpreter.retain(|p| seen.insert(p.clone()));
    if opts.python.interpreter.len() > 1 {
        bail!(
            "`maturin pep517 {cmd}` expects exactly one --interpreter, got {}: {:?}",
            opts.python.interpreter.len(),
            opts.python.interpreter,
        );
    }
    Ok(())
}

/// Dispatches into the native implementations of the PEP 517 functions
///
/// The last line of stdout is used as return value from the python part of the implementation
#[instrument(skip_all)]
pub fn pep517(subcommand: Pep517Command) -> Result<()> {
    // PEP 517 builds default to release profile.
    fn ensure_release_profile(context: &mut BuildContext) {
        if context.project.cargo_options.profile.is_none() {
            context.project.cargo_options.profile = Some("release".to_string());
        }
    }

    match subcommand {
        Pep517Command::WriteDistInfo {
            mut build_options,
            metadata_directory,
            strip_opt,
        } => {
            require_single_interpreter(&mut build_options, "write-dist-info")?;
            let mut context = build_options
                .into_build_context()
                .strip(strip_opt.strip)
                .editable(false)
                .build()?;
            ensure_release_profile(&mut context);

            let mut writer =
                VirtualWriter::new(PathWriter::from_path(metadata_directory), Override::empty());

            let orchestrator = BuildOrchestrator::new(&context);
            let dist_info_dir = write_dist_info(
                &mut writer,
                &context.project.project_layout.project_root,
                &context.project.metadata24,
                &orchestrator.tags_from_bridge()?,
            )?;
            writer.finish()?;
            println!("{}", dist_info_dir.display());
        }
        Pep517Command::BuildWheel {
            mut build_options,
            strip_opt,
            editable,
        } => {
            require_single_interpreter(&mut build_options, "build-wheel")?;
            let mut build_context = build_options
                .into_build_context()
                .strip(strip_opt.strip)
                .editable(editable)
                .build()?;
            ensure_release_profile(&mut build_context);

            let orchestrator = BuildOrchestrator::new(&build_context);
            let wheels = orchestrator.build_wheels()?;
            if wheels.len() != 1 {
                bail!(
                    "expected exactly one wheel to be built, got {}: {:?}",
                    wheels.len(),
                    wheels
                        .iter()
                        .map(|wheel| wheel.path.display().to_string())
                        .collect::<Vec<_>>(),
                );
            }
            let wheel_path = wheels[0].path.to_str().with_context(|| {
                format!(
                    "wheel path is not valid UTF-8: {}",
                    wheels[0].path.display()
                )
            })?;
            println!("{wheel_path}");
        }
        Pep517Command::WriteSDist {
            sdist_directory,
            manifest_path,
        } => {
            let build_options = BuildOptions {
                output: OutputOptions {
                    out: Some(sdist_directory),
                    ..Default::default()
                },
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
                .strip(Some(false))
                .editable(false)
                .sdist_only(true)
                .build()?;

            let orchestrator = BuildOrchestrator::new(&build_context);
            let sdist = orchestrator
                .build_source_distribution()?
                .context("Failed to build source distribution, pyproject.toml not found")?;
            println!("{}", sdist.path.file_name().unwrap().to_str().unwrap());
        }
    };

    Ok(())
}
