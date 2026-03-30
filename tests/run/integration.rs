use crate::common::integration::{self, IntegrationCase};
use crate::common::other;
use crate::common::{
    CFFI_MIXED_IMPLICIT_COPY, CFFI_MIXED_INCLUDE_EXCLUDE_COPY, CFFI_MIXED_PY_SUBDIR_COPY,
    CFFI_MIXED_SRC_COPY, CFFI_MIXED_SUBMODULE_COPY, CFFI_MIXED_WITH_PATH_DEP_COPY, handle_result,
    has_conda, has_uniffi_bindgen, is_ci, test_python_implementation,
};
use std::path::Path;

#[test]
fn integration_pyo3_bin() {
    let python_implementation = test_python_implementation().unwrap();
    if python_implementation == "pypy" || python_implementation == "graalpy" {
        // PyPy & GraalPy do not support the auto-initialize feature of pyo3.
        return;
    }

    handle_result(integration::test_integration(&IntegrationCase::new(
        "integration-pyo3-bin",
        "test-crates/pyo3-bin",
    )));
}

#[rstest::rstest]
#[case::pyo3_pure(IntegrationCase::new("integration-pyo3-pure", "test-crates/pyo3-pure"))]
#[case::pyo3_mixed(IntegrationCase::new("integration-pyo3-mixed", "test-crates/pyo3-mixed"))]
// Keep the old mixed-layout regression coverage after moving these integration cases from pyo3 to
// cffi. The fixtures generate package files in-tree, so each case runs from a copied workspace.
#[case::cffi_mixed_include_exclude(IntegrationCase::new(
    "integration-cffi-mixed-include-exclude",
    "test-crates/cffi-mixed-include-exclude",
).copied(CFFI_MIXED_INCLUDE_EXCLUDE_COPY))]
#[case::cffi_mixed_submodule(IntegrationCase::new(
    "integration-cffi-mixed-submodule",
    "test-crates/cffi-mixed-submodule",
).copied(CFFI_MIXED_SUBMODULE_COPY))]
#[case::cffi_mixed_with_path_dep(IntegrationCase::new(
    "integration-cffi-mixed-with-path-dep",
    "test-crates/cffi-mixed-with-path-dep",
).copied(CFFI_MIXED_WITH_PATH_DEP_COPY))]
#[case::cffi_mixed_implicit(IntegrationCase::new(
    "integration-cffi-mixed-implicit",
    "test-crates/cffi-mixed-implicit",
).copied(CFFI_MIXED_IMPLICIT_COPY))]
#[case::cffi_mixed_py_subdir({
    let case = IntegrationCase::new(
        "integration-cffi-mixed-py-subdir",
        "test-crates/cffi-mixed-py-subdir",
    ).copied(CFFI_MIXED_PY_SUBDIR_COPY);
    if cfg!(unix) { case.zig() } else { case }
})]
#[case::cffi_mixed_src_layout(IntegrationCase::new(
    "integration-cffi-mixed-src",
    "test-crates/cffi-mixed-src/rust",
).copied(CFFI_MIXED_SRC_COPY))]
#[case::uniffi_pure_proc_macro(IntegrationCase::new(
    "integration-uniffi-pure-proc-macro",
    "test-crates/uniffi-pure-proc-macro",
))]
#[case::hello_world(IntegrationCase::new("integration-hello-world", "test-crates/hello-world"))]
#[case::pyo3_ffi_pure(IntegrationCase::new(
    "integration-pyo3-ffi-pure",
    "test-crates/pyo3-ffi-pure"
))]
#[case::with_data(IntegrationCase::new("integration-with-data", "test-crates/with-data"))]
#[case::readme_duplication(IntegrationCase::new(
    "integration-readme-duplication",
    "test-crates/readme-duplication/readme-py",
))]
#[case::workspace_inverted_order(IntegrationCase::new(
    "integration-workspace-inverted-order",
    "test-crates/workspace-inverted-order/path-dep-with-root",
))]
#[case::pyo3_stub_generation(IntegrationCase::new(
    "integration-pyo3-stub-generation",
    "test-crates/pyo3-stub-generation",
).generate_stubs())]
#[cfg_attr(unix, case::pyo3_stub_generation_zig(IntegrationCase::new(
    "integration-pyo3-stub-generation-zig",
    "test-crates/pyo3-stub-generation",
).generate_stubs().zig()))]
#[test]
fn integration_cases(#[case] case: IntegrationCase<'_>) {
    handle_result(integration::test_integration(&case));
}

#[test]
#[cfg_attr(target_os = "macos", ignore)]
fn integration_pyo3_mixed_conda() {
    // Don't run it on macOS, too slow.
    if has_conda() {
        handle_result(integration::test_integration_conda(
            "test-crates/pyo3-mixed",
            None,
            "integration-pyo3-mixed-conda",
        ));
    }
}

#[rstest::rstest]
#[case::cffi_pure(IntegrationCase::new("integration-cffi-pure", "test-crates/cffi-pure"))]
#[case::cffi_mixed(IntegrationCase::new("integration-cffi-mixed", "test-crates/cffi-mixed"))]
#[test]
fn integration_cffi_cases(#[case] case: IntegrationCase<'_>) {
    if is_ci() && test_python_implementation().unwrap() == "pypy" {
        return;
    }
    handle_result(integration::test_integration(&case));
}

#[rstest::rstest]
#[case::uniffi_pure(IntegrationCase::new("integration-uniffi-pure", "test-crates/uniffi-pure"))]
#[case::uniffi_mixed(IntegrationCase::new("integration-uniffi-mixed", "test-crates/uniffi-mixed"))]
#[test]
fn integration_uniffi_cases(#[case] case: IntegrationCase<'_>) {
    if is_ci() || has_uniffi_bindgen() {
        handle_result(integration::test_integration(&case));
    }
}

#[test]
#[cfg(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        target_env = "gnu",
    ),
    all(
        target_os = "macos",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
))]
fn integration_wasm_hello_world() {
    handle_result(integration::test_integration(
        &IntegrationCase::new("integration-wasm-hello-world", "test-crates/hello-world")
            .target("wasm32-wasip1"),
    ));

    let python_implementation = test_python_implementation().unwrap();
    let venv_name =
        format!("integration-wasm-hello-world-py3-wasm32-wasip1-{python_implementation}");

    assert!(
        Path::new("test-crates")
            .join("venvs")
            .join(venv_name)
            .join(if cfg!(target_os = "windows") {
                "Scripts"
            } else {
                "bin"
            })
            .join("hello-world.wasm")
            .is_file()
    )
}

#[test]
fn abi3_without_version() {
    handle_result(other::abi3_without_version())
}

#[test]
fn abi3_python_interpreter_args() {
    handle_result(other::abi3_python_interpreter_args());
}
