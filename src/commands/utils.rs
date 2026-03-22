use anyhow::{Context, Result};
use maturin::{BuildOptions, BuildOrchestrator, UnpackedSdist, unpack_sdist};
use std::path::PathBuf;

/// Result of unpacking an sdist for wheel building
pub struct UnpackedBuild {
    /// Must be kept alive for the duration of the build
    _tmpdir: tempfile::TempDir,
    pub pyproject_toml_path: Option<PathBuf>,
}

/// Build a source distribution from the given build options, returning the sdist path.
pub fn build_sdist(build: &BuildOptions, strip: Option<bool>) -> Result<PathBuf> {
    let sdist_context = build
        .clone()
        .into_build_context()
        .strip(strip)
        .editable(false)
        .sdist_only(true)
        .build()?;

    let orchestrator = BuildOrchestrator::new(&sdist_context);
    let (sdist_path, _) = orchestrator
        .build_source_distribution()?
        .context("Failed to build source distribution, pyproject.toml not found")?;
    Ok(sdist_path)
}

/// Build an sdist, unpack it, and point build options at the unpacked source.
///
/// Returns the sdist path and the temporary directory holding the unpacked tree.
pub fn unpack_sdist_for_build(
    build: &mut BuildOptions,
    strip: Option<bool>,
) -> Result<(PathBuf, UnpackedBuild)> {
    let sdist_path = build_sdist(build, strip)?;
    // Preserve the original output directory so that wheels built
    // from the unpacked sdist still land in the user-visible
    // `target/wheels` (or the explicit `--out` directory) instead
    // of the temporary directory's target.
    if build.output.out.is_none() {
        build.output.out = sdist_path.parent().map(PathBuf::from);
    }
    let UnpackedSdist {
        tmpdir,
        cargo_toml,
        pyproject_toml,
    } = unpack_sdist(&sdist_path)?;
    eprintln!(
        "📦 Building wheels from source distribution at {}",
        cargo_toml.parent().unwrap().display()
    );
    build.cargo.manifest_path = Some(cargo_toml);
    Ok((
        sdist_path,
        UnpackedBuild {
            _tmpdir: tmpdir,
            pyproject_toml_path: Some(pyproject_toml),
        },
    ))
}
