use crate::common::integration::{self, IntegrationCase};
use crate::common::other;
use crate::common::{
    TestPackageCopy, handle_result, has_conda, has_uniffi_bindgen, is_ci,
    test_python_implementation,
};
use std::path::Path;

#[test]
fn integration_pyo3_bin() {
    let python_implementation = test_python_implementation().unwrap();
    if python_implementation == "pypy" || python_implementation == "graalpy" {
        // PyPy & GraalPy do not support the auto-initialize feature of pyo3.
        return;
    }

    handle_result(integration::test_integration(&IntegrationCase {
        id: "integration-pyo3-bin",
        package: "test-crates/pyo3-bin",
        package_copy: None,
        bindings: None,
        zig: false,
        target: None,
    }));
}

#[rstest::rstest]
#[case::pyo3_pure(IntegrationCase {
    id: "integration-pyo3-pure",
    package: "test-crates/pyo3-pure",
    package_copy: None,
    bindings: None,
    zig: false,
    target: None,
})]
#[case::pyo3_mixed(IntegrationCase {
    id: "integration-pyo3-mixed",
    package: "test-crates/pyo3-mixed",
    package_copy: None,
    bindings: None,
    zig: false,
    target: None,
})]
// Keep the old mixed-layout regression coverage after moving these integration cases from pyo3 to
// cffi. The fixtures generate package files in-tree, so each case runs from a copied workspace.
#[case::cffi_mixed_include_exclude(IntegrationCase {
    id: "integration-cffi-mixed-include-exclude",
    package: "test-crates/cffi-mixed-include-exclude",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &[],
        prune_copy_paths: &[
            "test-crates/cffi-mixed-include-exclude/cffi_mixed_include_exclude/cffi_mixed_include_exclude",
            "test-crates/cffi-mixed-include-exclude/cffi_mixed_include_exclude/generated_info.txt",
        ],
    }),
    bindings: None,
    zig: false,
    target: None,
})]
#[case::cffi_mixed_submodule(IntegrationCase {
    id: "integration-cffi-mixed-submodule",
    package: "test-crates/cffi-mixed-submodule",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &[],
        prune_copy_paths: &["test-crates/cffi-mixed-submodule/cffi_mixed_submodule/rust_module/rust"],
    }),
    bindings: None,
    zig: false,
    target: None,
})]
#[case::cffi_mixed_with_path_dep(IntegrationCase {
    id: "integration-cffi-mixed-with-path-dep",
    package: "test-crates/cffi-mixed-with-path-dep",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &["test-crates/some_path_dep", "test-crates/transitive_path_dep"],
        prune_copy_paths: &[
            "test-crates/cffi-mixed-with-path-dep/cffi_mixed_with_path_dep/cffi_mixed_with_path_dep",
        ],
    }),
    bindings: None,
    zig: false,
    target: None,
})]
#[case::cffi_mixed_implicit(IntegrationCase {
    id: "integration-cffi-mixed-implicit",
    package: "test-crates/cffi-mixed-implicit",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &[],
        prune_copy_paths: &["test-crates/cffi-mixed-implicit/python/cffi_mixed_implicit/some_rust/rust"],
    }),
    bindings: None,
    zig: false,
    target: None,
})]
#[case::cffi_mixed_py_subdir(IntegrationCase {
    id: "integration-cffi-mixed-py-subdir",
    package: "test-crates/cffi-mixed-py-subdir",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &[],
        prune_copy_paths: &["test-crates/cffi-mixed-py-subdir/python/cffi_mixed_py_subdir/_cffi_mixed"],
    }),
    bindings: None,
    zig: cfg!(unix),
    target: None,
})]
#[case::cffi_mixed_src_layout(IntegrationCase {
    id: "integration-cffi-mixed-src",
    package: "test-crates/cffi-mixed-src/rust",
    package_copy: Some(TestPackageCopy {
        extra_copy_paths: &[],
        prune_copy_paths: &["test-crates/cffi-mixed-src/src/cffi_mixed_src/cffi_mixed_src"],
    }),
    bindings: None,
    zig: false,
    target: None,
})]
#[case::uniffi_pure_proc_macro(IntegrationCase {
    id: "integration-uniffi-pure-proc-macro",
    package: "test-crates/uniffi-pure-proc-macro",
    package_copy: None,
    bindings: None,
    zig: false,
    target: None,
})]
#[case::hello_world(IntegrationCase {
    id: "integration-hello-world",
    package: "test-crates/hello-world",
    package_copy: None,
    bindings: None,
    zig: false,
    target: None,
})]
#[case::pyo3_ffi_pure(IntegrationCase {
    id: "integration-pyo3-ffi-pure",
    package: "test-crates/pyo3-ffi-pure",
    package_copy: None,
    bindings: None,
    zig: false,
    target: None,
})]
#[case::with_data(IntegrationCase {
    id: "integration-with-data",
    package: "test-crates/with-data",
    package_copy: None,
    bindings: None,
    zig: false,
    target: None,
})]
#[case::readme_duplication(IntegrationCase {
    id: "integration-readme-duplication",
    package: "test-crates/readme-duplication/readme-py",
    package_copy: None,
    bindings: None,
    zig: false,
    target: None,
})]
#[case::workspace_inverted_order(IntegrationCase {
    id: "integration-workspace-inverted-order",
    package: "test-crates/workspace-inverted-order/path-dep-with-root",
    package_copy: None,
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
    // Don't run it on macOS, too slow.
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
    package_copy: None,
    bindings: None,
    zig: false,
    target: None,
})]
#[case::cffi_mixed(IntegrationCase {
    id: "integration-cffi-mixed",
    package: "test-crates/cffi-mixed",
    package_copy: None,
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
    package_copy: None,
    bindings: None,
    zig: false,
    target: None,
})]
#[case::uniffi_mixed(IntegrationCase {
    id: "integration-uniffi-mixed",
    package: "test-crates/uniffi-mixed",
    package_copy: None,
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
        package_copy: None,
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
