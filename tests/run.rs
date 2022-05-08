//! To speed up the tests, they are tests all collected in a single module

use common::{develop, editable, errors, handle_result, integration, other};

mod common;

#[test]
fn develop_pyo3_pure() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-pure",
        None,
        "develop_pyo3_pure",
    ));
}

#[test]
fn develop_pyo3_mixed() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed",
        None,
        "develop_pyo3_mixed",
    ));
}

#[test]
fn develop_pyo3_mixed_submodule() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-submodule",
        None,
        "develop_pyo3_mixed_submodule",
    ));
}

#[test]
fn develop_pyo3_mixed_py_subdir() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-py-subdir",
        None,
        "develop_pyo3_mixed_py_subdir",
    ));
}

#[test]
fn develop_cffi_pure() {
    handle_result(develop::test_develop(
        "test-crates/cffi-pure",
        None,
        "develop_cffi_pure",
    ));
}

#[test]
fn develop_cffi_mixed() {
    handle_result(develop::test_develop(
        "test-crates/cffi-mixed",
        None,
        "develop_cffi_mixed",
    ));
}

#[test]
fn develop_hello_world() {
    handle_result(develop::test_develop(
        "test-crates/hello-world",
        None,
        "develop_hello_world",
    ));
}

#[test]
fn develop_pyo3_ffi_pure() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-ffi-pure",
        None,
        "develop_pyo3_ffi_pure",
    ));
}

#[test]
fn editable_pyo3_pure() {
    handle_result(editable::test_editable(
        "test-crates/pyo3-pure",
        None,
        "editable_pyo3_pure",
    ));
}

#[test]
fn editable_pyo3_mixed() {
    handle_result(editable::test_editable(
        "test-crates/pyo3-mixed",
        None,
        "editable_pyo3_mixed",
    ));
}

#[test]
fn editable_pyo3_mixed_py_subdir() {
    handle_result(editable::test_editable(
        "test-crates/pyo3-mixed-py-subdir",
        None,
        "editable_pyo3_mixed_py_subdir",
    ));
}

#[test]
fn editable_pyo3_ffi_pure() {
    handle_result(editable::test_editable(
        "test-crates/pyo3-ffi-pure",
        None,
        "editable_pyo3_ffi_pure",
    ));
}

#[test]
fn integration_pyo3_pure() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-pure",
        None,
        "integration_pyo3_pure",
        false,
    ));
}

#[test]
fn integration_pyo3_mixed() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-mixed",
        None,
        "integration_pyo3_mixed",
        false,
    ));
}

#[test]
fn integration_pyo3_mixed_submodule() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-mixed-submodule",
        None,
        "integration_pyo3_mixed_submodule",
        false,
    ));
}

#[test]
fn integration_pyo3_mixed_py_subdir() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-mixed-py-subdir",
        None,
        "integration_pyo3_mixed_py_subdir",
        cfg!(unix),
    ));
}

#[cfg(target_os = "windows")]
#[test]
#[ignore]
fn integration_pyo3_pure_conda() {
    handle_result(integration::test_integration_conda(
        "text-crates/pyo3-pure",
        None,
    ));
}

#[test]
fn integration_cffi_pure() {
    handle_result(integration::test_integration(
        "test-crates/cffi-pure",
        None,
        "integration_cffi_pure",
        false,
    ));
}

#[test]
fn integration_cffi_mixed() {
    handle_result(integration::test_integration(
        "test-crates/cffi-mixed",
        None,
        "integration_cffi_mixed",
        false,
    ));
}

#[test]
fn integration_hello_world() {
    handle_result(integration::test_integration(
        "test-crates/hello-world",
        None,
        "integration_hello_world",
        false,
    ));
}

#[test]
fn integration_pyo3_ffi_pure() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-ffi-pure",
        None,
        "integration_pyo3_ffi_pure",
        false,
    ));
}
#[test]
fn integration_with_data() {
    handle_result(integration::test_integration(
        "test-crates/with-data",
        None,
        "integration_with_data",
        false,
    ));
}

#[test]
fn abi3_without_version() {
    handle_result(errors::abi3_without_version())
}

#[test]
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn pyo3_no_extension_module() {
    handle_result(errors::pyo3_no_extension_module())
}

#[test]
fn locked_doesnt_build_without_cargo_lock() {
    handle_result(errors::locked_doesnt_build_without_cargo_lock())
}

#[test]
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn invalid_manylinux_does_not_panic() {
    handle_result(errors::invalid_manylinux_does_not_panic())
}

#[test]
#[cfg(target_os = "linux")]
fn musl() {
    let ran = handle_result(other::test_musl());
    if !ran {
        eprintln!("⚠️  Warning: rustup and/or musl target not installed, test didn't run");
    }
}

#[test]
fn workspace_cargo_lock() {
    handle_result(other::test_workspace_cargo_lock())
}

#[test]
fn lib_with_path_dep_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/sdist_with_path_dep",
        vec![
            "sdist_with_path_dep-0.1.0/local_dependencies/some_path_dep/Cargo.toml",
            "sdist_with_path_dep-0.1.0/local_dependencies/some_path_dep/src/lib.rs",
            "sdist_with_path_dep-0.1.0/local_dependencies/transitive_path_dep/Cargo.toml",
            "sdist_with_path_dep-0.1.0/local_dependencies/transitive_path_dep/src/lib.rs",
            "sdist_with_path_dep-0.1.0/Cargo.toml",
            "sdist_with_path_dep-0.1.0/pyproject.toml",
            "sdist_with_path_dep-0.1.0/src/lib.rs",
            "sdist_with_path_dep-0.1.0/PKG-INFO",
        ],
        "lib_with_path_dep_sdist",
    ))
}

#[test]
fn workspace_with_path_dep_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/workspace_with_path_dep/python",
        vec![
            "workspace_with_path_dep-0.1.0/local_dependencies/generic_lib/Cargo.toml",
            "workspace_with_path_dep-0.1.0/local_dependencies/generic_lib/src/lib.rs",
            "workspace_with_path_dep-0.1.0/local_dependencies/transitive_lib/Cargo.toml",
            "workspace_with_path_dep-0.1.0/local_dependencies/transitive_lib/src/lib.rs",
            "workspace_with_path_dep-0.1.0/Cargo.toml",
            "workspace_with_path_dep-0.1.0/pyproject.toml",
            "workspace_with_path_dep-0.1.0/src/lib.rs",
            "workspace_with_path_dep-0.1.0/PKG-INFO",
        ],
        "workspace_with_path_dep_sdist",
    ))
}
