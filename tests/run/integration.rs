use crate::common::integration::{self, IntegrationCase};
use crate::common::other;
use crate::common::{
    handle_result, has_conda, has_uniffi_bindgen, is_ci, test_python_implementation,
};
use std::path::Path;

#[test]
fn integration_pyo3_bin() {
    let python_implementation = test_python_implementation().unwrap();
    if python_implementation == "pypy" || python_implementation == "graalpy" {
        return;
    }

    handle_result(integration::test_integration(&IntegrationCase {
        id: "integration-pyo3-bin",
        package: "test-crates/pyo3-bin",
        bindings: None,
        zig: false,
        target: None,
    }));
}

#[rstest::rstest]
#[case::pyo3_pure(IntegrationCase {
    id: "integration-pyo3-pure",
    package: "test-crates/pyo3-pure",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::pyo3_mixed(IntegrationCase {
    id: "integration-pyo3-mixed",
    package: "test-crates/pyo3-mixed",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::pyo3_mixed_include_exclude(IntegrationCase {
    id: "integration-pyo3-mixed-include-exclude",
    package: "test-crates/pyo3-mixed-include-exclude",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::pyo3_mixed_submodule(IntegrationCase {
    id: "integration-pyo3-mixed-submodule",
    package: "test-crates/pyo3-mixed-submodule",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::pyo3_mixed_with_path_dep(IntegrationCase {
    id: "integration-pyo3-mixed-with-path-dep",
    package: "test-crates/pyo3-mixed-with-path-dep",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::pyo3_mixed_implicit(IntegrationCase {
    id: "integration-pyo3-mixed-implicit",
    package: "test-crates/pyo3-mixed-implicit",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::pyo3_mixed_py_subdir(IntegrationCase {
    id: "integration-pyo3-mixed-py-subdir",
    package: "test-crates/pyo3-mixed-py-subdir",
    bindings: None,
    zig: cfg!(unix),
    target: None,
})]
#[case::pyo3_mixed_src_layout(IntegrationCase {
    id: "integration-pyo3-mixed-src",
    package: "test-crates/pyo3-mixed-src/rust",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::uniffi_pure_proc_macro(IntegrationCase {
    id: "integration-uniffi-pure-proc-macro",
    package: "test-crates/uniffi-pure-proc-macro",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::hello_world(IntegrationCase {
    id: "integration-hello-world",
    package: "test-crates/hello-world",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::pyo3_ffi_pure(IntegrationCase {
    id: "integration-pyo3-ffi-pure",
    package: "test-crates/pyo3-ffi-pure",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::with_data(IntegrationCase {
    id: "integration-with-data",
    package: "test-crates/with-data",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::readme_duplication(IntegrationCase {
    id: "integration-readme-duplication",
    package: "test-crates/readme-duplication/readme-py",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::workspace_inverted_order(IntegrationCase {
    id: "integration-workspace-inverted-order",
    package: "test-crates/workspace-inverted-order/path-dep-with-root",
    bindings: None,
    zig: false,
    target: None,
})]
#[test]
fn integration_cases(#[case] case: IntegrationCase<'_>) {
    handle_result(integration::test_integration(&case));
}

#[test]
#[cfg_attr(target_os = "macos", ignore)]
fn integration_pyo3_pure_conda() {
    if has_conda() {
        handle_result(integration::test_integration_conda(
            "test-crates/pyo3-mixed",
            None,
            "integration-pyo3-pure-conda",
        ));
    }
}

#[rstest::rstest]
#[case::cffi_pure(IntegrationCase {
    id: "integration-cffi-pure",
    package: "test-crates/cffi-pure",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::cffi_mixed(IntegrationCase {
    id: "integration-cffi-mixed",
    package: "test-crates/cffi-mixed",
    bindings: None,
    zig: false,
    target: None,
})]
#[test]
fn integration_cffi_cases(#[case] case: IntegrationCase<'_>) {
    if is_ci() && test_python_implementation().unwrap() == "pypy" {
        return;
    }
    handle_result(integration::test_integration(&case));
}

#[rstest::rstest]
#[case::uniffi_pure(IntegrationCase {
    id: "integration-uniffi-pure",
    package: "test-crates/uniffi-pure",
    bindings: None,
    zig: false,
    target: None,
})]
#[case::uniffi_mixed(IntegrationCase {
    id: "integration-uniffi-mixed",
    package: "test-crates/uniffi-mixed",
    bindings: None,
    zig: false,
    target: None,
})]
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
    handle_result(integration::test_integration(&IntegrationCase {
        id: "integration-wasm-hello-world",
        package: "test-crates/hello-world",
        bindings: None,
        zig: false,
        target: Some("wasm32-wasip1"),
    }));

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
