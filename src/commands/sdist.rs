use anyhow::{Context, Result};
use maturin::{BuildOptions, BuildOrchestrator, CargoOptions, OutputOptions, find_path_deps};
use std::path::PathBuf;
use tracing::instrument;

#[instrument(skip_all)]
pub fn sdist(manifest_path: Option<PathBuf>, out: Option<PathBuf>) -> Result<()> {
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
        output: OutputOptions {
            out,
            ..Default::default()
        },
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
        .strip(Some(false))
        .editable(false)
        .sdist_only(true)
        .build()?;

    let orchestrator = BuildOrchestrator::new(&build_context);
    orchestrator
        .build_source_distribution()?
        .context("Failed to build source distribution, pyproject.toml not found")?;
    Ok(())
}
