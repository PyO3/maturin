//! To speed up the tests, they are tests all collected in a single module

use common::{
    develop, editable, errors, get_python_implementation, handle_result, integration, other,
    test_python_path,
};
use indoc::indoc;
use maturin::Target;
use std::path::{Path, PathBuf};

mod common;

#[test]
fn develop_pyo3_pure() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-pure",
        None,
        "develop-pyo3-pure",
        false,
    ));
}

#[test]
#[ignore]
fn develop_pyo3_pure_conda() {
    // Only run on GitHub Actions for now
    if std::env::var("GITHUB_ACTIONS").is_ok() {
        handle_result(develop::test_develop(
            "test-crates/pyo3-pure",
            None,
            "develop-pyo3-pure-conda",
            true,
        ));
    }
}

#[test]
fn develop_pyo3_mixed() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed",
        None,
        "develop-pyo3-mixed",
        false,
    ));
}

#[test]
fn develop_pyo3_mixed_include_exclude() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-include-exclude",
        None,
        "develop-pyo3-mixed-include-exclude",
        false,
    ));
}

#[test]
fn develop_pyo3_mixed_submodule() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-submodule",
        None,
        "develop-pyo3-mixed-submodule",
        false,
    ));
}

#[test]
fn develop_pyo3_mixed_py_subdir() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-py-subdir",
        None,
        "develop-pyo3-mixed-py-subdir",
        false,
    ));
}

#[test]
fn develop_pyo3_mixed_src_layout() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-src/rust",
        None,
        "develop-pyo3-mixed-src",
        false,
    ));
}

#[test]
fn develop_cffi_pure() {
    handle_result(develop::test_develop(
        "test-crates/cffi-pure",
        None,
        "develop-cffi-pure",
        false,
    ));
}

#[test]
fn develop_cffi_mixed() {
    handle_result(develop::test_develop(
        "test-crates/cffi-mixed",
        None,
        "develop-cffi-mixed",
        false,
    ));
}

#[test]
fn develop_hello_world() {
    handle_result(develop::test_develop(
        "test-crates/hello-world",
        None,
        "develop-hello-world",
        false,
    ));
}

#[test]
fn develop_pyo3_ffi_pure() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-ffi-pure",
        None,
        "develop-pyo3-ffi-pure",
        false,
    ));
}

#[test]
fn editable_pyo3_pure() {
    handle_result(editable::test_editable(
        "test-crates/pyo3-pure",
        None,
        "editable-pyo3-pure",
    ));
}

#[test]
fn editable_pyo3_mixed() {
    handle_result(editable::test_editable(
        "test-crates/pyo3-mixed",
        None,
        "editable-pyo3-mixed",
    ));
}

#[test]
fn editable_pyo3_mixed_py_subdir() {
    handle_result(editable::test_editable(
        "test-crates/pyo3-mixed-py-subdir",
        None,
        "editable-pyo3-mixed-py-subdir",
    ));
}

#[test]
fn editable_pyo3_ffi_pure() {
    handle_result(editable::test_editable(
        "test-crates/pyo3-ffi-pure",
        None,
        "editable-pyo3-ffi-pure",
    ));
}

#[test]
fn integration_pyo3_bin() {
    let python = test_python_path().map(PathBuf::from).unwrap_or_else(|| {
        let target = Target::from_target_triple(None).unwrap();
        target.get_python()
    });
    let python_implementation = get_python_implementation(&python).unwrap();
    if python_implementation == "pypy" {
        // PyPy doesn't support the 'auto-initialize' feature of pyo3
        return;
    }

    handle_result(integration::test_integration(
        "test-crates/pyo3-bin",
        None,
        "integration-pyo3-bin",
        false,
        None,
    ));
}

#[test]
fn integration_pyo3_pure() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-pure",
        None,
        "integration-pyo3-pure",
        false,
        None,
    ));
}

#[test]
fn integration_pyo3_mixed() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-mixed",
        None,
        "integration-pyo3-mixed",
        false,
        None,
    ));
}

#[test]
fn integration_pyo3_mixed_include_exclude() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-mixed-include-exclude",
        None,
        "integration-pyo3-mixed-include-exclude",
        false,
        None,
    ));
}

#[test]
fn integration_pyo3_mixed_submodule() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-mixed-submodule",
        None,
        "integration-pyo3-mixed-submodule",
        false,
        None,
    ));
}

#[test]
fn integration_pyo3_mixed_py_subdir() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-mixed-py-subdir",
        None,
        "integration-pyo3-mixed-py-subdir",
        cfg!(unix),
        None,
    ));
}

#[test]
fn integration_pyo3_mixed_src_layout() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-mixed-src/rust",
        None,
        "integration-pyo3-mixed-src",
        false,
        None,
    ));
}

#[test]
#[cfg_attr(target_os = "macos", ignore)] // Don't run it on macOS, too slow
fn integration_pyo3_pure_conda() {
    // Only run on GitHub Actions for now
    if std::env::var("GITHUB_ACTIONS").is_ok() {
        handle_result(integration::test_integration_conda(
            "test-crates/pyo3-mixed",
            None,
        ));
    }
}

#[test]
fn integration_cffi_pure() {
    handle_result(integration::test_integration(
        "test-crates/cffi-pure",
        None,
        "integration-cffi-pure",
        false,
        None,
    ));
}

#[test]
fn integration_cffi_mixed() {
    handle_result(integration::test_integration(
        "test-crates/cffi-mixed",
        None,
        "integration-cffi-mixed",
        false,
        None,
    ));
}

#[test]
fn integration_hello_world() {
    handle_result(integration::test_integration(
        "test-crates/hello-world",
        None,
        "integration-hello-world",
        false,
        None,
    ));
}

#[test]
fn integration_pyo3_ffi_pure() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-ffi-pure",
        None,
        "integration-pyo3-ffi-pure",
        false,
        None,
    ));
}

#[test]
fn integration_with_data() {
    handle_result(integration::test_integration(
        "test-crates/with-data",
        None,
        "integration-with-data",
        false,
        None,
    ));
}

#[test]
// Sourced from https://pypi.org/project/wasmtime/2.0.0/#files
// update with wasmtime updates
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
    use std::path::Path;

    handle_result(integration::test_integration(
        "test-crates/hello-world",
        None,
        "integration-wasm-hello-world",
        false,
        Some("wasm32-wasi"),
    ));

    let python = test_python_path().map(PathBuf::from).unwrap_or_else(|| {
        let target = Target::from_target_triple(None).unwrap();
        target.get_python()
    });
    let python_implementation = get_python_implementation(&python).unwrap();
    let venv_name = format!(
        "integration-wasm-hello-world-py3-wasm32-wasi-{}",
        python_implementation
    );

    // Make sure we're actually running wasm
    assert!(Path::new("test-crates")
        .join("venvs")
        .join(venv_name)
        .join(if cfg!(target_os = "windows") {
            "Scripts"
        } else {
            "bin"
        })
        .join("hello-world.wasm")
        .is_file())
}

#[test]
fn abi3_without_version() {
    handle_result(errors::abi3_without_version())
}

#[test]
#[cfg_attr(not(all(target_os = "linux", target_env = "gnu")), ignore)]
fn pyo3_no_extension_module() {
    let python = test_python_path().map(PathBuf::from).unwrap_or_else(|| {
        let target = Target::from_target_triple(None).unwrap();
        target.get_python()
    });
    let python_implementation = get_python_implementation(&python).unwrap();
    if python_implementation == "cpython" {
        handle_result(errors::pyo3_no_extension_module())
    }
}

#[test]
fn locked_doesnt_build_without_cargo_lock() {
    handle_result(errors::locked_doesnt_build_without_cargo_lock())
}

#[test]
#[cfg_attr(not(all(target_os = "linux", target_env = "gnu")), ignore)]
fn invalid_manylinux_does_not_panic() {
    handle_result(errors::invalid_manylinux_does_not_panic())
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
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
fn workspace_members_non_local_dep_sdist() {
    let cargo_toml = indoc!(
        r#"
        [package]
        authors = ["konstin <konstin@mailbox.org>"]
        name = "pyo3-pure"
        version = "2.1.2"
        edition = "2021"
        description = "Implements a dummy function (get_fortytwo.DummyClass.get_42()) in rust"
        license = "MIT"

        [dependencies]
        pyo3 = { version = "0.17.3", features = ["abi3-py37", "extension-module", "generate-import-lib"] }

        [lib]
        name = "pyo3_pure"
        crate-type = ["cdylib"]
        "#
    );
    handle_result(other::test_source_distribution(
        "test-crates/pyo3-pure",
        vec![
            "pyo3_pure-0.1.0+abc123de/Cargo.lock",
            "pyo3_pure-0.1.0+abc123de/Cargo.toml",
            "pyo3_pure-0.1.0+abc123de/LICENSE",
            "pyo3_pure-0.1.0+abc123de/PKG-INFO",
            "pyo3_pure-0.1.0+abc123de/README.md",
            "pyo3_pure-0.1.0+abc123de/check_installed/check_installed.py",
            "pyo3_pure-0.1.0+abc123de/pyo3_pure.pyi",
            "pyo3_pure-0.1.0+abc123de/pyproject.toml",
            "pyo3_pure-0.1.0+abc123de/src/lib.rs",
            "pyo3_pure-0.1.0+abc123de/tests/test_pyo3_pure.py",
            "pyo3_pure-0.1.0+abc123de/tox.ini",
        ],
        Some((Path::new("pyo3_pure-0.1.0+abc123de/Cargo.toml"), cargo_toml)),
        "sdist-workspace-members-non-local-dep",
    ))
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
            "sdist_with_path_dep-0.1.0/Cargo.lock",
            "sdist_with_path_dep-0.1.0/pyproject.toml",
            "sdist_with_path_dep-0.1.0/src/lib.rs",
            "sdist_with_path_dep-0.1.0/PKG-INFO",
        ],
        None,
        "sdist-lib-with-path-dep",
    ))
}

#[test]
fn pyo3_mixed_src_layout_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/pyo3-mixed-src/rust",
        vec![
            "pyo3_mixed_src-2.1.3/pyproject.toml",
            "pyo3_mixed_src-2.1.3/src/pyo3_mixed_src/__init__.py",
            "pyo3_mixed_src-2.1.3/src/pyo3_mixed_src/python_module/__init__.py",
            "pyo3_mixed_src-2.1.3/src/pyo3_mixed_src/python_module/double.py",
            "pyo3_mixed_src-2.1.3/rust/Cargo.toml",
            "pyo3_mixed_src-2.1.3/rust/Cargo.lock",
            "pyo3_mixed_src-2.1.3/rust/src/lib.rs",
            "pyo3_mixed_src-2.1.3/PKG-INFO",
        ],
        None,
        "sdist-pyo3-mixed-src-layout",
    ))
}

#[test]
fn pyo3_mixed_include_exclude_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/pyo3-mixed-include-exclude",
        vec![
            // "pyo3_mixed_include_exclude-2.1.3/.gitignore", // excluded
            "pyo3_mixed_include_exclude-2.1.3/Cargo.lock",
            "pyo3_mixed_include_exclude-2.1.3/Cargo.toml",
            "pyo3_mixed_include_exclude-2.1.3/PKG-INFO",
            "pyo3_mixed_include_exclude-2.1.3/README.md",
            "pyo3_mixed_include_exclude-2.1.3/check_installed/check_installed.py",
            // "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/exclude_this_file, excluded
            "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/__init__.py",
            "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/include_this_file", // included
            "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/python_module/__init__.py",
            "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/python_module/double.py",
            "pyo3_mixed_include_exclude-2.1.3/pyproject.toml",
            "pyo3_mixed_include_exclude-2.1.3/src/lib.rs",
            // "pyo3_mixed_include_exclude-2.1.3/tests/test_pyo3_mixed_include_exclude.py", excluded
            "pyo3_mixed_include_exclude-2.1.3/tox.ini",
        ],
        None,
        "sdist-pyo3-mixed-include-exclude",
    ))
}

#[test]
fn pyo3_mixed_include_exclude_wheel_files() {
    handle_result(other::check_wheel_files(
        "test-crates/pyo3-mixed-include-exclude",
        vec![
            "pyo3_mixed_include_exclude-2.1.3.dist-info/METADATA",
            "pyo3_mixed_include_exclude-2.1.3.dist-info/RECORD",
            "pyo3_mixed_include_exclude-2.1.3.dist-info/WHEEL",
            "pyo3_mixed_include_exclude-2.1.3.dist-info/entry_points.txt",
            "pyo3_mixed_include_exclude/__init__.py",
            "pyo3_mixed_include_exclude/include_this_file",
            "pyo3_mixed_include_exclude/python_module/__init__.py",
            "pyo3_mixed_include_exclude/python_module/double.py",
            "README.md",
        ],
        "wheel-files-pyo3-mixed-include-exclude",
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
        None,
        "sdist-workspace-with-path-dep",
    ))
}

#[rustversion::since(1.64)]
#[test]
fn workspace_inheritance_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/workspace-inheritance/python",
        vec![
            "workspace_inheritance-0.1.0/local_dependencies/generic_lib/Cargo.toml",
            "workspace_inheritance-0.1.0/local_dependencies/generic_lib/src/lib.rs",
            "workspace_inheritance-0.1.0/Cargo.toml",
            "workspace_inheritance-0.1.0/pyproject.toml",
            "workspace_inheritance-0.1.0/src/lib.rs",
            "workspace_inheritance-0.1.0/PKG-INFO",
        ],
        None,
        "sdist-workspace-inheritance",
    ))
}

#[test]
fn abi3_python_interpreter_args() {
    handle_result(other::abi3_python_interpreter_args());
}
