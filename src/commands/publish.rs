use crate::commands::utils::unpack_sdist_for_build;
use anyhow::Result;
use maturin::{BuildOptions, PublishOpt, upload_ui};

pub fn publish(
    mut build: BuildOptions,
    mut publish: PublishOpt,
    debug: bool,
    no_strip: bool,
    no_sdist: bool,
    pgo: bool,
) -> Result<()> {
    // set profile to dev if specified; `--debug` and `--profile` are mutually exclusive
    //
    // do it here to take precedence over pyproject.toml profile setting
    if debug {
        build.profile = Some("dev".to_string());
    }

    // Keep tempdir alive for the duration of the build
    let _sdist_tmp;
    let mut sdist_path = None;
    let sdist_pyproject_path;
    if !no_sdist {
        let (path, unpacked) = unpack_sdist_for_build(&mut build, Some(!no_strip))?;
        sdist_pyproject_path = unpacked.pyproject_toml_path.clone();
        _sdist_tmp = Some(unpacked);
        sdist_path = Some(path);
    } else {
        _sdist_tmp = None;
        sdist_pyproject_path = None;
    }

    let mut build_context = build
        .into_build_context()
        .strip(Some(!no_strip))
        .editable(false)
        .pyproject_toml_path(sdist_pyproject_path)
        .pgo(pgo)
        .build()?;

    // ensure profile always set when publishing
    // (respect pyproject.toml if set)
    // don't need to check `debug` here, set above to take precedence if set
    let profile = build_context
        .project
        .cargo_options
        .profile
        .get_or_insert_with(|| "release".to_string());
    if profile == "dev" {
        eprintln!("⚠️  Warning: You're publishing debug wheels");
    }

    let mut wheels = build_context.build_wheels()?;
    if let Some(sdist_path) = sdist_path {
        wheels.push((sdist_path, "source".to_string()));
    }

    let items = wheels.into_iter().map(|wheel| wheel.0).collect::<Vec<_>>();
    publish.non_interactive_on_ci();
    upload_ui(&items, &publish)?;
    Ok(())
}
