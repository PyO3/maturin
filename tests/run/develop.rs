use crate::common::develop::{self, DevelopCase};
use crate::common::{
    CFFI_MIXED_IMPLICIT_COPY, CFFI_MIXED_INCLUDE_EXCLUDE_COPY, CFFI_MIXED_PY_SUBDIR_COPY,
    CFFI_MIXED_SRC_COPY, CFFI_MIXED_SUBMODULE_COPY, CFFI_MIXED_WITH_PATH_DEP_COPY, handle_result,
    has_conda, has_uniffi_bindgen, has_uv, is_ci, test_python_implementation,
};
use rstest::rstest;
use std::time::Duration;

#[rstest]
#[case::pyo3_pure(DevelopCase::pip("develop-pyo3-pure", "test-crates/pyo3-pure"))]
#[case::pyo3_mixed(DevelopCase::pip("develop-pyo3-mixed", "test-crates/pyo3-mixed"))]
// Keep the old mixed-layout regression coverage after moving these develop cases from pyo3 to
// cffi. The fixtures generate package files in-tree, so each case runs from a copied workspace.
#[case::cffi_mixed_include_exclude(DevelopCase::pip(
    "develop-cffi-mixed-include-exclude",
    "test-crates/cffi-mixed-include-exclude",
).copied(CFFI_MIXED_INCLUDE_EXCLUDE_COPY))]
#[case::cffi_mixed_submodule(DevelopCase::pip(
    "develop-cffi-mixed-submodule",
    "test-crates/cffi-mixed-submodule",
).copied(CFFI_MIXED_SUBMODULE_COPY))]
#[case::cffi_mixed_with_path_dep(DevelopCase::pip(
    "develop-cffi-mixed-with-path-dep",
    "test-crates/cffi-mixed-with-path-dep",
).copied(CFFI_MIXED_WITH_PATH_DEP_COPY))]
#[case::cffi_mixed_implicit(DevelopCase::pip(
    "develop-cffi-mixed-implicit",
    "test-crates/cffi-mixed-implicit",
).copied(CFFI_MIXED_IMPLICIT_COPY))]
#[case::cffi_mixed_py_subdir(DevelopCase::pip(
    "develop-cffi-mixed-py-subdir",
    "test-crates/cffi-mixed-py-subdir",
).copied(CFFI_MIXED_PY_SUBDIR_COPY))]
#[case::cffi_mixed_src_layout(DevelopCase::pip(
    "develop-cffi-mixed-src",
    "test-crates/cffi-mixed-src/rust",
).copied(CFFI_MIXED_SRC_COPY))]
#[case::uniffi_pure_proc_macro(DevelopCase::pip(
    "develop-uniffi-pure-proc-macro",
    "test-crates/uniffi-pure-proc-macro",
))]
#[case::uniffi_multiple_crates(DevelopCase::pip(
    "develop-uniffi-multiple-crates",
    "test-crates/uniffi-multiple-crates",
))]
/// Test editable install of a project with both a binary and a Python module.
/// This is a regression test for https://github.com/PyO3/maturin/issues/2933
#[case::bin_with_python_module(DevelopCase::pip(
    "develop-bin-with-python-module",
    "test-crates/bin-with-python-module",
))]
#[test]
fn develop_pip_cases(#[case] case: DevelopCase<'_>) {
    handle_result(develop::test_develop(&case));
}

#[test]
fn develop_pyo3_pure_conda() {
    if has_conda() {
        handle_result(develop::test_develop(
            &DevelopCase::pip("develop-pyo3-pure-conda", "test-crates/pyo3-pure").conda(3, 10),
        ));
    }
}

#[rstest]
#[case::cffi_pure(DevelopCase::pip(
    "develop-cffi-pure",
    "test-crates/cffi-pure",
).prereqs(&["cffi"]))]
#[case::cffi_mixed(DevelopCase::pip(
    "develop-cffi-mixed",
    "test-crates/cffi-mixed",
).prereqs(&["cffi"]))]
#[test]
fn develop_cffi_cases(#[case] case: DevelopCase<'_>) {
    if is_ci() && test_python_implementation().unwrap() == "pypy" {
        return;
    }
    handle_result(develop::test_develop(&case));
}

#[rstest]
#[case::uniffi_pure(DevelopCase::pip("develop-uniffi-pure", "test-crates/uniffi-pure"))]
#[case::uniffi_mixed(DevelopCase::pip("develop-uniffi-mixed", "test-crates/uniffi-mixed"))]
#[case::uniffi_multiple_binding_files(DevelopCase::pip(
    "develop-uniffi-multiple-binding-files",
    "test-crates/uniffi-multiple-binding-files",
))]
#[test]
fn develop_uniffi_cases(#[case] case: DevelopCase<'_>) {
    if is_ci() || has_uniffi_bindgen() {
        handle_result(develop::test_develop(&case));
    }
}

#[rstest]
#[timeout(Duration::from_secs(120))]
#[case::hello_world(DevelopCase::uv("develop-hello-world-uv", "test-crates/hello-world",))]
#[case::pyo3_ffi_pure(DevelopCase::uv("develop-pyo3-ffi-pure-uv", "test-crates/pyo3-ffi-pure",))]
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
#[case::hello_world(DevelopCase::pip("develop-hello-world-pip", "test-crates/hello-world",))]
#[case::pyo3_ffi_pure(DevelopCase::pip("develop-pyo3-ffi-pure-pip", "test-crates/pyo3-ffi-pure",))]
#[test]
fn develop_backend_parameterized_cases(#[case] case: DevelopCase<'_>) {
    handle_result(develop::test_develop(&case));
}
