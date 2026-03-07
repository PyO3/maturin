use crate::common::{
    PreparedEnv, TestEnvKind, TestInstallBackend, case_target_dir, check_installed, has_uv,
    manifest_path_for_package, prepare_test_env,
};
use anyhow::Result;
use maturin::{CargoOptions, DevelopOptions, develop};
use std::path::Path;

#[derive(Clone, Copy)]
pub struct DevelopCase<'a> {
    pub id: &'a str,
    pub package: &'a str,
    pub bindings: Option<&'a str>,
    pub env_kind: TestEnvKind,
    pub backend: TestInstallBackend,
    pub prereq_packages: &'a [&'a str],
}

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// "maturin develop" to install it and checks it is working
pub fn test_develop(case: &DevelopCase<'_>) -> Result<()> {
    let package = Path::new(case.package);
    let PreparedEnv {
        root: venv_dir,
        python,
    } = prepare_test_env(case.id, case.env_kind, case.prereq_packages, None)?;

    // Ensure the test doesn't wrongly pass
    check_installed(package, &python).unwrap_err();

    let uv = matches!(case.backend, TestInstallBackend::Uv);
    if uv
        && !cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        ))
    {
        assert!(has_uv(), "uv backend requires uv binary on this platform");
    }

    let develop_options = DevelopOptions {
        bindings: case.bindings.map(|binding| binding.to_owned()),
        release: false,
        strip: false,
        extras: Vec::new(),
        group: Vec::new(),
        skip_install: false,
        pip_path: None,
        cargo_options: CargoOptions {
            manifest_path: Some(manifest_path_for_package(package)),
            quiet: true,
            target_dir: Some(case_target_dir(case.id)),
            ..Default::default()
        },
        uv,
        compression: Default::default(),
    };
    develop(develop_options, &venv_dir)?;

    check_installed(package, &python)?;
    Ok(())
}
