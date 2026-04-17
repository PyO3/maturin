use crate::commands::StripOption;
use crate::commands::utils::unpack_sdist_for_build;
use anyhow::Result;
use maturin::{BuildOptions, BuildOrchestrator};
use tracing::instrument;

#[instrument(skip_all)]
pub fn build(
    mut build: BuildOptions,
    release: bool,
    strip_opt: StripOption,
    sdist: bool,
    pgo: bool,
) -> Result<()> {
    let strip = strip_opt.strip;
    // set profile to release if specified; `--release` and `--profile` are mutually exclusive
    if release {
        build.profile = Some("release".to_string());
    }
    // Keep tempdir alive for the duration of the build
    let _sdist_tmp;
    let sdist_pyproject_path;
    if sdist {
        let (_, unpacked) = unpack_sdist_for_build(&mut build, strip)?;
        sdist_pyproject_path = unpacked.pyproject_toml_path.clone();
        _sdist_tmp = Some(unpacked);
    } else {
        _sdist_tmp = None;
        sdist_pyproject_path = None;
    }
    let build_context = build
        .into_build_context()
        .strip(strip)
        .editable(false)
        .pyproject_toml_path(sdist_pyproject_path)
        .pgo(pgo)
        .build()?;

    let orchestrator = BuildOrchestrator::new(&build_context);
    let wheels = orchestrator.build_wheels()?;
    if wheels.is_empty() {
        anyhow::bail!("No wheels were built");
    }
    Ok(())
}
