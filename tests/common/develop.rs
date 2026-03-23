use crate::common::{
    PreparedEnv, TestEnvKind, TestInstallBackend, TestPackageCopy, case_target_dir,
    check_installed, cleanup_case, has_uv, manifest_path_for_package, prepare_case_package,
    prepare_test_env,
};
use anyhow::Result;
use maturin::{CargoOptions, DevelopOptions, develop};

/// A table-driven `maturin develop` scenario.
///
/// The case id is used to derive isolated virtualenv and cargo target paths, so it should stay
/// stable and descriptive when possible.
#[derive(Clone, Copy)]
pub struct DevelopCase<'a> {
    /// Stable identifier used for derived test paths and failure messages.
    pub id: &'a str,
    /// Repo-relative path to the package under test.
    pub package: &'a str,
    /// Optional copied-workspace configuration for fixtures that generate files in-tree.
    pub package_copy: Option<TestPackageCopy<'a>>,
    /// Optional explicit bindings override passed to `maturin develop`.
    pub bindings: Option<&'a str>,
    /// The environment kind used for installation and verification.
    pub env_kind: TestEnvKind,
    /// Whether the case installs through pip-compatible or uv-compatible flow.
    pub backend: TestInstallBackend,
    /// Extra Python packages that must be installed into the test environment first.
    pub prereq_packages: &'a [&'a str],
}

impl<'a> DevelopCase<'a> {
    pub fn pip(id: &'a str, package: &'a str) -> Self {
        Self {
            id,
            package,
            package_copy: None,
            bindings: None,
            env_kind: TestEnvKind::Venv,
            backend: TestInstallBackend::Pip,
            prereq_packages: &[],
        }
    }

    pub fn uv(id: &'a str, package: &'a str) -> Self {
        Self {
            backend: TestInstallBackend::Uv,
            prereq_packages: &["uv"],
            ..Self::pip(id, package)
        }
    }

    pub fn copied(mut self, copy: TestPackageCopy<'a>) -> Self {
        self.package_copy = Some(copy);
        self
    }

    pub fn prereqs(mut self, packages: &'a [&'a str]) -> Self {
        self.prereq_packages = packages;
        self
    }

    pub fn conda(mut self, major: usize, minor: usize) -> Self {
        self.env_kind = TestEnvKind::Conda { major, minor };
        self
    }
}

/// Creates a virtualenv and activates it, checks that the package isn't installed, uses
/// "maturin develop" to install it and checks it is working
pub fn test_develop(case: &DevelopCase<'_>) -> Result<()> {
    let package_path = prepare_case_package(case.id, case.package, case.package_copy)?;
    let package = package_path.as_path();
    let uv = matches!(case.backend, TestInstallBackend::Uv);
    let supported_uv_platform = cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ));
    let prereq_packages = if uv && !supported_uv_platform && has_uv() {
        &[][..]
    } else {
        case.prereq_packages
    };
    let PreparedEnv {
        env_dir: venv_dir,
        python,
    } = prepare_test_env(case.id, case.env_kind, prereq_packages, None)?;

    // Ensure the test doesn't wrongly pass
    check_installed(package, &python).unwrap_err();

    if uv && !supported_uv_platform {
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
        generate_stubs: false,
    };
    develop(develop_options, &venv_dir)?;

    check_installed(package, &python)?;
    cleanup_case(case.id);
    Ok(())
}
