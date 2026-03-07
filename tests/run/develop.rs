use crate::common::develop::{self, DevelopCase};
use crate::common::{
    TestEnvKind, TestInstallBackend, handle_result, has_conda, has_uniffi_bindgen, has_uv, is_ci,
    test_python_implementation,
};
use rstest::rstest;
use std::time::Duration;

#[rstest]
#[case::pyo3_pure(DevelopCase {
    id: "develop-pyo3-pure",
    package: "test-crates/pyo3-pure",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::pyo3_mixed(DevelopCase {
    id: "develop-pyo3-mixed",
    package: "test-crates/pyo3-mixed",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::pyo3_mixed_include_exclude(DevelopCase {
    id: "develop-pyo3-mixed-include-exclude",
    package: "test-crates/pyo3-mixed-include-exclude",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::pyo3_mixed_submodule(DevelopCase {
    id: "develop-pyo3-mixed-submodule",
    package: "test-crates/pyo3-mixed-submodule",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::pyo3_mixed_with_path_dep(DevelopCase {
    id: "develop-pyo3-mixed-with-path-dep",
    package: "test-crates/pyo3-mixed-with-path-dep",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::pyo3_mixed_implicit(DevelopCase {
    id: "develop-pyo3-mixed-implicit",
    package: "test-crates/pyo3-mixed-implicit",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::pyo3_mixed_py_subdir(DevelopCase {
    id: "develop-pyo3-mixed-py-subdir",
    package: "test-crates/pyo3-mixed-py-subdir",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::pyo3_mixed_src_layout(DevelopCase {
    id: "develop-pyo3-mixed-src",
    package: "test-crates/pyo3-mixed-src/rust",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::uniffi_pure_proc_macro(DevelopCase {
    id: "develop-uniffi-pure-proc-macro",
    package: "test-crates/uniffi-pure-proc-macro",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::uniffi_multiple_crates(DevelopCase {
    id: "develop-uniffi-multiple-crates",
    package: "test-crates/uniffi-multiple-crates",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::bin_with_python_module(DevelopCase {
    id: "develop-bin-with-python-module",
    package: "test-crates/bin-with-python-module",
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
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &["cffi"],
})]
#[case::cffi_mixed(DevelopCase {
    id: "develop-cffi-mixed",
    package: "test-crates/cffi-mixed",
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
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::uniffi_mixed(DevelopCase {
    id: "develop-uniffi-mixed",
    package: "test-crates/uniffi-mixed",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::uniffi_multiple_binding_files(DevelopCase {
    id: "develop-uniffi-multiple-binding-files",
    package: "test-crates/uniffi-multiple-binding-files",
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
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Uv,
    prereq_packages: &["uv"],
})]
#[case::pyo3_ffi_pure(DevelopCase {
    id: "develop-pyo3-ffi-pure-uv",
    package: "test-crates/pyo3-ffi-pure",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Uv,
    prereq_packages: &["uv"],
})]
#[test]
fn develop_uv_cases(#[case] case: DevelopCase<'_>) {
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
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[case::pyo3_ffi_pure(DevelopCase {
    id: "develop-pyo3-ffi-pure-pip",
    package: "test-crates/pyo3-ffi-pure",
    bindings: None,
    env_kind: TestEnvKind::Venv,
    backend: TestInstallBackend::Pip,
    prereq_packages: &[],
})]
#[test]
fn develop_backend_parameterized_cases(#[case] case: DevelopCase<'_>) {
    handle_result(develop::test_develop(&case));
}
