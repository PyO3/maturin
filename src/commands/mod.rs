#[cfg(feature = "zig")]
use anyhow::Context;
use anyhow::Result;
#[cfg(feature = "schemars")]
use maturin::GenerateJsonSchemaOptions;
#[cfg(feature = "upload")]
use maturin::PublishOpt;
use maturin::{BridgeModel, Target, TargetTriple};
#[cfg(feature = "scaffolding")]
use maturin::{GenerateProjectOptions, ci::GenerateCI};
#[cfg(feature = "upload")]
use std::path::PathBuf;
use tracing::instrument;

pub(crate) mod build;
pub(crate) mod develop;
pub(crate) mod pep517;
#[cfg(feature = "upload")]
pub(crate) mod publish;
pub(crate) mod sdist;
pub(crate) mod utils;

/// Shared `--strip` CLI option used by multiple commands.
#[derive(Debug, clap::Args, Clone, Copy)]
pub struct StripOption {
    /// Strip the library for minimum file size.
    /// Can be set to `true` or `false`, or used as a flag (`--strip` implies `true`).
    #[arg(
        long,
        env = "MATURIN_STRIP",
        // `--strip` without a value is treated as `--strip true`
        default_missing_value = "true",
        num_args = 0..=1,
        require_equals = false
    )]
    pub strip: Option<bool>,
}

/// Generate shell completions
#[cfg(feature = "cli-completion")]
#[instrument(skip_all)]
pub fn completions(shell: clap_complete_command::Shell, cmd: &mut clap::Command) {
    shell.generate(cmd, &mut std::io::stdout());
}

/// Search and list the available python installations
#[instrument(skip_all)]
pub fn list_python(target: Option<TargetTriple>) -> Result<()> {
    let found = if target.is_some() {
        let target = Target::from_target_triple(target.as_ref())?;
        maturin::PythonInterpreter::lookup_target(&target, None, None)
    } else {
        let target = Target::from_target_triple(None)?;
        // We don't know the targeted bindings yet, so we use the most lenient
        maturin::PythonInterpreter::find_all(&target, &BridgeModel::Cffi, None)?
    };
    eprintln!("🐍 {} python interpreter found:", found.len());
    for interpreter in found {
        eprintln!(" - {interpreter}");
    }
    Ok(())
}

/// Create a new cargo project in an existing directory
#[cfg(feature = "scaffolding")]
#[instrument(skip_all)]
pub fn init_project(path: Option<String>, options: GenerateProjectOptions) -> Result<()> {
    maturin::init_project(path, options)
}

/// Create a new cargo project
#[cfg(feature = "scaffolding")]
#[instrument(skip_all)]
pub fn new_project(path: String, options: GenerateProjectOptions) -> Result<()> {
    maturin::new_project(path, options)
}

/// Generate CI configuration
#[cfg(feature = "scaffolding")]
#[instrument(skip_all)]
pub fn generate_ci(generate_ci: GenerateCI) -> Result<()> {
    generate_ci.execute()
}

/// Generate the JSON schema for the `pyproject.toml` file.
#[cfg(feature = "schemars")]
#[instrument(skip_all)]
pub fn generate_json_schema(args: GenerateJsonSchemaOptions) -> Result<()> {
    maturin::generate_json_schema(args)
}

/// Upload python packages to pypi
#[cfg(feature = "upload")]
#[instrument(skip_all)]
pub fn upload(mut publish: PublishOpt, files: Vec<PathBuf>) -> Result<()> {
    if files.is_empty() {
        eprintln!("⚠️  Warning: No files given, exiting.");
        return Ok(());
    }
    publish.non_interactive_on_ci();

    maturin::upload_ui(&files, &publish)?;
    Ok(())
}

/// Zig linker wrapper
#[cfg(feature = "zig")]
#[instrument(skip_all)]
pub fn zig(subcommand: cargo_zigbuild::Zig) -> Result<()> {
    subcommand
        .execute()
        .context("Failed to run zig linker wrapper")
}
