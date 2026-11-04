use anyhow::{Context, Result};
use maturin::{BuildOptions, BuildOrchestrator, CargoOptions, OutputOptions};
use std::path::PathBuf;
use tracing::instrument;

#[instrument(skip_all)]
pub fn sdist(manifest_path: Option<PathBuf>, out: Option<PathBuf>) -> Result<()> {
    // Inspect declared dependencies without resolving them: this probe runs
    // before the locked/frozen options in pyproject.toml have been loaded.
    let checked_manifest = manifest_path
        .as_deref()
        .unwrap_or_else(|| std::path::Path::new("Cargo.toml"));
    let cargo_metadata_result = cargo_metadata::MetadataCommand::new()
        .cargo_path("cargo")
        .manifest_path(checked_manifest)
        .no_deps()
        .verbose(true)
        .exec();

    let has_path_deps = cargo_metadata_result
        .ok()
        .and_then(|metadata| {
            let checked_manifest = fs_err::canonicalize(checked_manifest).ok()?;
            metadata.packages.into_iter().find(|package| {
                fs_err::canonicalize(&package.manifest_path).ok().as_ref()
                    == Some(&checked_manifest)
            })
        })
        .map(|package| package.dependencies.iter().any(|dep| dep.path.is_some()))
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
