use crate::common::develop::{self, DevelopCase};
use crate::common::{
    TestEnvKind, TestInstallBackend, TestPackageCopy, handle_result, has_conda, has_uniffi_bindgen,
    has_uv, is_ci, test_python_implementation,
};
use rstest::rstest;
use std::time::Duration;

#[rstest]
#[case::pyo3_pure(DevelopCase {
    id: "develop-pyo3-pure",
    package: "test-crates/pyo3-pure",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::pyo3_mixed(DevelopCase {
    id: "develop-pyo3-mixed",
    package: "test-crates/pyo3-mixed",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
// Keep the old mixed-layout regression coverage after moving these develop cases from pyo3 to
// cffi. The fixtures generate package files in-tree, so each case runs from a copied workspace.
#[case::cffi_mixed_include_exclude(DevelopCase {
    id: "develop-cffi-mixed-include-exclude",
    package: "test-crates/cffi-mixed-include-exclude",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &[],
        prune_copy_paths: &[
            "test-crates/cffi-mixed-include-exclude/cffi_mixed_include_exclude/cffi_mixed_include_exclude",
            "test-crates/cffi-mixed-include-exclude/cffi_mixed_include_exclude/generated_info.txt",
        ],
    }),
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::cffi_mixed_submodule(DevelopCase {
    id: "develop-cffi-mixed-submodule",
    package: "test-crates/cffi-mixed-submodule",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &[],
        prune_copy_paths: &["test-crates/cffi-mixed-submodule/cffi_mixed_submodule/rust_module/rust"],
    }),
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::cffi_mixed_with_path_dep(DevelopCase {
    id: "develop-cffi-mixed-with-path-dep",
    package: "test-crates/cffi-mixed-with-path-dep",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &["test-crates/some_path_dep", "test-crates/transitive_path_dep"],
        prune_copy_paths: &[
            "test-crates/cffi-mixed-with-path-dep/cffi_mixed_with_path_dep/cffi_mixed_with_path_dep",
        ],
    }),
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::cffi_mixed_implicit(DevelopCase {
    id: "develop-cffi-mixed-implicit",
    package: "test-crates/cffi-mixed-implicit",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &[],
        prune_copy_paths: &["test-crates/cffi-mixed-implicit/python/cffi_mixed_implicit/some_rust/rust"],
    }),
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::cffi_mixed_py_subdir(DevelopCase {
    id: "develop-cffi-mixed-py-subdir",
    package: "test-crates/cffi-mixed-py-subdir",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &[],
        prune_copy_paths: &["test-crates/cffi-mixed-py-subdir/python/cffi_mixed_py_subdir/_cffi_mixed"],
    }),
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::cffi_mixed_src_layout(DevelopCase {
    id: "develop-cffi-mixed-src",
    package: "test-crates/cffi-mixed-src/rust",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &[],
        prune_copy_paths: &["test-crates/cffi-mixed-src/src/cffi_mixed_src/cffi_mixed_src"],
    }),
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::uniffi_pure_proc_macro(DevelopCase {
    id: "develop-uniffi-pure-proc-macro",
    package: "test-crates/uniffi-pure-proc-macro",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::uniffi_multiple_crates(DevelopCase {
    id: "develop-uniffi-multiple-crates",
    package: "test-crates/uniffi-multiple-crates",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
/// Test editable install of a project with both a binary and a Python module.
/// This is a regression test for https://github.com/PyO3/maturin/issues/2933
#[case::bin_with_python_module(DevelopCase {
    id: "develop-bin-with-python-module",
    package: "test-crates/bin-with-python-module",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[test]
fn develop_pip_cases(#[case] case: DevelopCase<'_>) {
    handle_result(develop::test_develop(&case));
}

#[test]
fn develop_pyo3_pure_conda() {
    if has_conda() {
        handle_result(develop::test_develop(&DevelopCase {
            id: "develop-pyo3-pure-conda",
            package: "test-crates/pyo3-pure",
            package_copy: None,
            bindings: None,
            env_kind: TestEnvKind::Conda {
                major: 3,
                minor: 10,
            },
            backend: TestInstallBackend::Pip,
            prereq_packages: &[],
        }));
    }
}

#[rstest]
#[case::cffi_pure(DevelopCase {
    id: "develop-cffi-pure",
    package: "test-crates/cffi-pure",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &["cffi"],
})]
#[case::cffi_mixed(DevelopCase {
    id: "develop-cffi-mixed",
    package: "test-crates/cffi-mixed",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &["cffi"],
})]
#[test]
fn develop_cffi_cases(#[case] case: DevelopCase<'_>) {
    if is_ci() && test_python_implementation().unwrap() == "pypy" {
        return;
    }
    handle_result(develop::test_develop(&case));
}

#[rstest]
#[case::uniffi_pure(DevelopCase {
    id: "develop-uniffi-pure",
    package: "test-crates/uniffi-pure",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::uniffi_mixed(DevelopCase {
    id: "develop-uniffi-mixed",
    package: "test-crates/uniffi-mixed",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::uniffi_multiple_binding_files(DevelopCase {
    id: "develop-uniffi-multiple-binding-files",
    package: "test-crates/uniffi-multiple-binding-files",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[test]
fn develop_uniffi_cases(#[case] case: DevelopCase<'_>) {
    if is_ci() || has_uniffi_bindgen() {
        handle_result(develop::test_develop(&case));
    }
}

#[rstest]
#[timeout(Duration::from_secs(120))]
#[case::hello_world(DevelopCase {
    id: "develop-hello-world-uv",
    package: "test-crates/hello-world",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Uv,
    prereq_packages: &["uv"],
})]
#[case::pyo3_ffi_pure(DevelopCase {
    id: "develop-pyo3-ffi-pure-uv",
    package: "test-crates/pyo3-ffi-pure",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Uv,
    prereq_packages: &["uv"],
})]
#[test]
fn develop_uv_cases(#[case] case: DevelopCase<'_>) {
    // Only run uv tests on platforms that have wheels on PyPI or when a uv binary is found.
    if !cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    )) && !has_uv()
    {
        return;
    }
    handle_result(develop::test_develop(&case));
}

#[rstest]
#[timeout(Duration::from_secs(120))]
#[case::hello_world(DevelopCase {
    id: "develop-hello-world-pip",
    package: "test-crates/hello-world",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::pyo3_ffi_pure(DevelopCase {
    id: "develop-pyo3-ffi-pure-pip",
    package: "test-crates/pyo3-ffi-pure",
    package_copy: None,
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[test]
fn develop_backend_parameterized_cases(#[case] case: DevelopCase<'_>) {
    handle_result(develop::test_develop(&case));
}
