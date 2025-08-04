//! To speed up the tests, they are tests all collected in a single module

use common::{
    develop, errors, handle_result, integration, other, test_python_implementation,
    TestInstallBackend,
};
use expect_test::expect;
use maturin::pyproject_toml::SdistGenerator;
use rstest::rstest;
use std::env;
use std::path::Path;
use std::time::Duration;
use time::macros::datetime;
use which::which;

mod common;

#[test]
fn develop_pyo3_pure() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-pure",
        None,
        "develop-pyo3-pure",
        false,
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_pyo3_pure_conda() {
    if which("conda").is_ok() {
        handle_result(develop::test_develop(
            "test-crates/pyo3-pure",
            None,
            "develop-pyo3-pure-conda",
            true,
            TestInstallBackend::Pip,
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
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_pyo3_mixed_include_exclude() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-include-exclude",
        None,
        "develop-pyo3-mixed-include-exclude",
        false,
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_pyo3_mixed_submodule() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-submodule",
        None,
        "develop-pyo3-mixed-submodule",
        false,
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_pyo3_mixed_with_path_dep() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-with-path-dep",
        None,
        "develop-pyo3-mixed-with-path-dep",
        false,
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_pyo3_mixed_implicit() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-implicit",
        None,
        "develop-pyo3-mixed-implicit",
        false,
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_pyo3_mixed_py_subdir() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-py-subdir",
        None,
        "develop-pyo3-mixed-py-subdir",
        false,
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_pyo3_mixed_src_layout() {
    handle_result(develop::test_develop(
        "test-crates/pyo3-mixed-src/rust",
        None,
        "develop-pyo3-mixed-src",
        false,
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_cffi_pure() {
    let python_implementation = test_python_implementation().unwrap();
    if env::var("GITHUB_ACTIONS").is_ok() && python_implementation == "pypy" {
        // TODO: PyPy hangs on cffi test sometimes
        return;
    }
    handle_result(develop::test_develop(
        "test-crates/cffi-pure",
        None,
        "develop-cffi-pure",
        false,
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_cffi_mixed() {
    let python_implementation = test_python_implementation().unwrap();
    if env::var("GITHUB_ACTIONS").is_ok() && python_implementation == "pypy" {
        // PyPy hangs on cffi test sometimes
        return;
    }
    handle_result(develop::test_develop(
        "test-crates/cffi-mixed",
        None,
        "develop-cffi-mixed",
        false,
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_uniffi_pure() {
    if env::var("GITHUB_ACTIONS").is_ok() || which("uniffi-bindgen").is_ok() {
        handle_result(develop::test_develop(
            "test-crates/uniffi-pure",
            None,
            "develop-uniffi-pure",
            false,
            TestInstallBackend::Pip,
        ));
    }
}

#[test]
fn develop_uniffi_pure_proc_macro() {
    handle_result(develop::test_develop(
        "test-crates/uniffi-pure-proc-macro",
        None,
        "develop-uniffi-pure-proc-macro",
        false,
        TestInstallBackend::Pip,
    ));
}

#[test]
fn develop_uniffi_mixed() {
    if env::var("GITHUB_ACTIONS").is_ok() || which("uniffi-bindgen").is_ok() {
        handle_result(develop::test_develop(
            "test-crates/uniffi-mixed",
            None,
            "develop-uniffi-mixed",
            false,
            TestInstallBackend::Pip,
        ));
    }
}

#[test]
fn develop_uniffi_multiple_binding_files() {
    if env::var("GITHUB_ACTIONS").is_ok() || which("uniffi-bindgen").is_ok() {
        handle_result(develop::test_develop(
            "test-crates/uniffi-multiple-binding-files",
            None,
            "develop-uniffi-multiple-binding-files",
            false,
            TestInstallBackend::Pip,
        ));
    }
}

#[rstest]
#[timeout(Duration::from_secs(60))]
#[case(TestInstallBackend::Pip, "pip")]
#[case(TestInstallBackend::Uv, "uv")]
#[test]
fn develop_hello_world(#[case] backend: TestInstallBackend, #[case] name: &str) {
    // Only run uv tests on platforms that has wheel on PyPI or when uv binary is found
    if matches!(backend, TestInstallBackend::Uv)
        && !cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        ))
        && which("uv").is_err()
    {
        return;
    }

    handle_result(develop::test_develop(
        "test-crates/hello-world",
        None,
        format!("develop-hello-world-{name}").as_str(),
        false,
        backend,
    ));
}

#[rstest]
#[timeout(Duration::from_secs(120))]
#[case(TestInstallBackend::Pip, "pip")]
#[case(TestInstallBackend::Uv, "uv")]
#[test]
fn develop_pyo3_ffi_pure(#[case] backend: TestInstallBackend, #[case] name: &str) {
    // Only run uv tests on platforms that has wheel on PyPI or when uv binary is found
    if matches!(backend, TestInstallBackend::Uv)
        && !cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        ))
        && which("uv").is_err()
    {
        return;
    }

    handle_result(develop::test_develop(
        "test-crates/pyo3-ffi-pure",
        None,
        format!("develop-pyo3-ffi-pure-{name}").as_str(),
        false,
        backend,
    ));
}

#[test]
fn integration_pyo3_bin() {
    let python_implementation = test_python_implementation().unwrap();
    if python_implementation == "pypy" || python_implementation == "graalpy" {
        // PyPy & GraalPy do not support the 'auto-initialize' feature of pyo3
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
fn integration_pyo3_mixed_with_path_dep() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-mixed-with-path-dep",
        None,
        "integration-pyo3-mixed-with-path-dep",
        false,
        None,
    ));
}

#[test]
fn integration_pyo3_mixed_implicit() {
    handle_result(integration::test_integration(
        "test-crates/pyo3-mixed-implicit",
        None,
        "integration-pyo3-mixed-implicit",
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
    if which("conda").is_ok() {
        handle_result(integration::test_integration_conda(
            "test-crates/pyo3-mixed",
            None,
        ));
    }
}

#[test]
fn integration_cffi_pure() {
    let python_implementation = test_python_implementation().unwrap();
    if env::var("GITHUB_ACTIONS").is_ok() && python_implementation == "pypy" {
        // PyPy hangs on cffi test sometimes
        return;
    }
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
    let python_implementation = test_python_implementation().unwrap();
    if env::var("GITHUB_ACTIONS").is_ok() && python_implementation == "pypy" {
        // PyPy hangs on cffi test sometimes
        return;
    }
    handle_result(integration::test_integration(
        "test-crates/cffi-mixed",
        None,
        "integration-cffi-mixed",
        false,
        None,
    ));
}

#[test]
fn integration_uniffi_pure() {
    if env::var("GITHUB_ACTIONS").is_ok() || which("uniffi-bindgen").is_ok() {
        handle_result(integration::test_integration(
            "test-crates/uniffi-pure",
            None,
            "integration-uniffi-pure",
            false,
            None,
        ));
    }
}

#[test]
fn integration_uniffi_pure_proc_macro() {
    handle_result(integration::test_integration(
        "test-crates/uniffi-pure-proc-macro",
        None,
        "integration-uniffi-pure-proc-macro",
        false,
        None,
    ));
}

#[test]
fn integration_uniffi_mixed() {
    if env::var("GITHUB_ACTIONS").is_ok() || which("uniffi-bindgen").is_ok() {
        handle_result(integration::test_integration(
            "test-crates/uniffi-mixed",
            None,
            "integration-uniffi-mixed",
            false,
            None,
        ));
    }
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
fn integration_readme_duplication() {
    handle_result(integration::test_integration(
        "test-crates/readme-duplication/readme-py",
        None,
        "integration-readme-duplication",
        false,
        None,
    ));
}

#[test]
fn integration_workspace_inverted_order() {
    handle_result(integration::test_integration(
        "test-crates/workspace-inverted-order/path-dep-with-root",
        None,
        "integration-workspace-inverted-order",
        false,
        None,
    ));
}

#[test]
// Sourced from https://pypi.org/project/wasmtime/11.0.0/#files
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
        Some("wasm32-wasip1"),
    ));

    let python_implementation = test_python_implementation().unwrap();
    let venv_name =
        format!("integration-wasm-hello-world-py3-wasm32-wasip1-{python_implementation}");

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
    handle_result(other::abi3_without_version())
}

#[test]
// Only run this test on platforms that has manylinux support
#[cfg_attr(
    not(all(
        target_os = "linux",
        target_env = "gnu",
        any(
            target_arch = "x86",
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "powerpc64",
            target_arch = "s390x",
            target_arch = "arm"
        )
    )),
    ignore
)]
fn pyo3_no_extension_module() {
    let python_implementation = test_python_implementation().unwrap();
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
fn warn_on_missing_python_source() {
    handle_result(errors::warn_on_missing_python_source())
}

#[test]
fn pypi_compatibility_unsupported_target() {
    handle_result(errors::pypi_compatibility_unsupported_target())
}

#[test]
fn pypi_compatibility_mixed_tags() {
    handle_result(errors::pypi_compatibility_mixed_tags())
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
fn workspace_members_beneath_pyproject_sdist() {
    let cargo_toml = expect![[r#"
        [workspace]
        resolver = "2"
        members = ["pyo3-mixed-workspace", "python/pyo3-mixed-workspace-py"]
        "#]];
    handle_result(other::test_source_distribution(
        "test-crates/pyo3-mixed-workspace/rust/python/pyo3-mixed-workspace-py",
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "pyo3_mixed_workspace-2.1.3/PKG-INFO",
                "pyo3_mixed_workspace-2.1.3/pyproject.toml",
                "pyo3_mixed_workspace-2.1.3/rust/Cargo.lock",
                "pyo3_mixed_workspace-2.1.3/rust/Cargo.toml",
                "pyo3_mixed_workspace-2.1.3/rust/pyo3-mixed-workspace/Cargo.toml",
                "pyo3_mixed_workspace-2.1.3/rust/pyo3-mixed-workspace/src/lib.rs",
                "pyo3_mixed_workspace-2.1.3/rust/python/pyo3-mixed-workspace-py/Cargo.toml",
                "pyo3_mixed_workspace-2.1.3/rust/python/pyo3-mixed-workspace-py/src/lib.rs",
                "pyo3_mixed_workspace-2.1.3/src/pyo3_mixed_workspace/__init__.py",
                "pyo3_mixed_workspace-2.1.3/src/pyo3_mixed_workspace/python_module/__init__.py",
                "pyo3_mixed_workspace-2.1.3/src/pyo3_mixed_workspace/python_module/double.py",
                "pyo3_mixed_workspace-2.1.3/src/tests/test_pyo3_mixed.py",
            }
        "#]],
        Some((
            Path::new("pyo3_mixed_workspace-2.1.3/rust/Cargo.toml"),
            cargo_toml,
        )),
        "sdist-workspace-members-beneath_pyproject",
    ))
}

#[test]
fn workspace_members_non_local_dep_sdist() {
    let cargo_toml = expect![[r#"
        [package]
        authors = ["konstin <konstin@mailbox.org>"]
        name = "pyo3-pure"
        version = "2.1.2"
        edition = "2021"
        description = "Implements a dummy function (get_fortytwo.DummyClass.get_42()) in rust"
        license = "MIT"
        readme = "README.md"

        [dependencies]
        pyo3 = { version = "0.25.0", features = [
            "abi3-py37",
            "extension-module",
            "generate-import-lib",
        ] }

        [lib]
        name = "pyo3_pure"
        crate-type = ["cdylib"]
    "#]];
    handle_result(other::test_source_distribution(
        "test-crates/pyo3-pure",
        SdistGenerator::Cargo,
        expect![[r#"
            {
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
            }
        "#]],
        Some((Path::new("pyo3_pure-0.1.0+abc123de/Cargo.toml"), cargo_toml)),
        "sdist-workspace-members-non-local-dep",
    ))
}

#[test]
fn lib_with_path_dep_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/sdist_with_path_dep",
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "sdist_with_path_dep-0.1.0/PKG-INFO",
                "sdist_with_path_dep-0.1.0/pyproject.toml",
                "sdist_with_path_dep-0.1.0/sdist_with_path_dep/Cargo.lock",
                "sdist_with_path_dep-0.1.0/sdist_with_path_dep/Cargo.toml",
                "sdist_with_path_dep-0.1.0/sdist_with_path_dep/src/lib.rs",
                "sdist_with_path_dep-0.1.0/some_path_dep/Cargo.toml",
                "sdist_with_path_dep-0.1.0/some_path_dep/src/lib.rs",
                "sdist_with_path_dep-0.1.0/transitive_path_dep/Cargo.toml",
                "sdist_with_path_dep-0.1.0/transitive_path_dep/src/lib.rs",
            }
        "#]],
        None,
        "sdist-lib-with-path-dep",
    ))
}

#[test]
fn lib_with_target_path_dep_sdist() {
    let cargo_toml = expect![[r#"
        [package]
        name = "sdist_with_target_path_dep"
        version = "0.1.0"
        authors = ["konstin <konstin@mailbox.org>"]
        edition = "2021"

        [lib]
        crate-type = ["cdylib"]

        [dependencies]
        pyo3 = { version = "0.25.0", features = ["extension-module"] }

        [target.'cfg(not(target_endian = "all-over-the-place"))'.dependencies]
        some_path_dep = { path = "../some_path_dep" }
    "#]];
    handle_result(other::test_source_distribution(
        "test-crates/sdist_with_target_path_dep",
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "sdist_with_target_path_dep-0.1.0/PKG-INFO",
                "sdist_with_target_path_dep-0.1.0/pyproject.toml",
                "sdist_with_target_path_dep-0.1.0/sdist_with_target_path_dep/Cargo.lock",
                "sdist_with_target_path_dep-0.1.0/sdist_with_target_path_dep/Cargo.toml",
                "sdist_with_target_path_dep-0.1.0/sdist_with_target_path_dep/src/lib.rs",
                "sdist_with_target_path_dep-0.1.0/some_path_dep/Cargo.toml",
                "sdist_with_target_path_dep-0.1.0/some_path_dep/src/lib.rs",
                "sdist_with_target_path_dep-0.1.0/transitive_path_dep/Cargo.toml",
                "sdist_with_target_path_dep-0.1.0/transitive_path_dep/src/lib.rs",
            }
        "#]],
        Some((
            Path::new("sdist_with_target_path_dep-0.1.0/sdist_with_target_path_dep/Cargo.toml"),
            cargo_toml,
        )),
        "sdist-lib-with-target-path-dep",
    ))
}

#[test]
fn pyo3_mixed_src_layout_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/pyo3-mixed-src/rust",
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "pyo3_mixed_src-2.1.3/PKG-INFO",
                "pyo3_mixed_src-2.1.3/pyproject.toml",
                "pyo3_mixed_src-2.1.3/rust/Cargo.lock",
                "pyo3_mixed_src-2.1.3/rust/Cargo.toml",
                "pyo3_mixed_src-2.1.3/rust/src/lib.rs",
                "pyo3_mixed_src-2.1.3/src/pyo3_mixed_src/__init__.py",
                "pyo3_mixed_src-2.1.3/src/pyo3_mixed_src/python_module/__init__.py",
                "pyo3_mixed_src-2.1.3/src/pyo3_mixed_src/python_module/double.py",
                "pyo3_mixed_src-2.1.3/src/tests/test_pyo3_mixed.py",
            }
        "#]],
        None,
        "sdist-pyo3-mixed-src-layout",
    ))
}

#[test]
fn pyo3_mixed_include_exclude_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/pyo3-mixed-include-exclude",
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "pyo3_mixed_include_exclude-2.1.3/.gitignore",
                "pyo3_mixed_include_exclude-2.1.3/Cargo.lock",
                "pyo3_mixed_include_exclude-2.1.3/Cargo.toml",
                "pyo3_mixed_include_exclude-2.1.3/PKG-INFO",
                "pyo3_mixed_include_exclude-2.1.3/README.md",
                "pyo3_mixed_include_exclude-2.1.3/check_installed/check_installed.py",
                "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/__init__.py",
                "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/include_this_file",
                "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/python_module/__init__.py",
                "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/python_module/double.py",
                "pyo3_mixed_include_exclude-2.1.3/pyproject.toml",
                "pyo3_mixed_include_exclude-2.1.3/src/lib.rs",
                "pyo3_mixed_include_exclude-2.1.3/tox.ini",
            }
        "#]],
        None,
        "sdist-pyo3-mixed-include-exclude",
    ))
}

#[test]
fn pyo3_mixed_include_exclude_git_sdist_generator() {
    if !Path::new(".git").exists() {
        return;
    }
    handle_result(other::test_source_distribution(
        "test-crates/pyo3-mixed-include-exclude",
        SdistGenerator::Git,
        expect![[r#"
            {
                "pyo3_mixed_include_exclude-2.1.3/.gitignore",
                "pyo3_mixed_include_exclude-2.1.3/Cargo.lock",
                "pyo3_mixed_include_exclude-2.1.3/Cargo.toml",
                "pyo3_mixed_include_exclude-2.1.3/PKG-INFO",
                "pyo3_mixed_include_exclude-2.1.3/README.md",
                "pyo3_mixed_include_exclude-2.1.3/check_installed/check_installed.py",
                "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/__init__.py",
                "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/include_this_file",
                "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/python_module/__init__.py",
                "pyo3_mixed_include_exclude-2.1.3/pyo3_mixed_include_exclude/python_module/double.py",
                "pyo3_mixed_include_exclude-2.1.3/pyproject.toml",
                "pyo3_mixed_include_exclude-2.1.3/src/lib.rs",
                "pyo3_mixed_include_exclude-2.1.3/tox.ini",
            }
        "#]],
        None,
        "sdist-pyo3-mixed-include-exclude-git",
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
fn workspace_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/workspace/py",
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "py-0.1.0/Cargo.lock",
                "py-0.1.0/Cargo.toml",
                "py-0.1.0/PKG-INFO",
                "py-0.1.0/py/Cargo.toml",
                "py-0.1.0/py/src/main.rs",
                "py-0.1.0/pyproject.toml",
            }
        "#]],
        None,
        "sdist-workspace",
    ))
}

#[test]
fn workspace_with_path_dep_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/workspace_with_path_dep/python",
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "workspace_with_path_dep-0.1.0/Cargo.lock",
                "workspace_with_path_dep-0.1.0/Cargo.toml",
                "workspace_with_path_dep-0.1.0/PKG-INFO",
                "workspace_with_path_dep-0.1.0/generic_lib/Cargo.toml",
                "workspace_with_path_dep-0.1.0/generic_lib/src/lib.rs",
                "workspace_with_path_dep-0.1.0/pyproject.toml",
                "workspace_with_path_dep-0.1.0/python/Cargo.toml",
                "workspace_with_path_dep-0.1.0/python/src/lib.rs",
                "workspace_with_path_dep-0.1.0/transitive_lib/Cargo.toml",
                "workspace_with_path_dep-0.1.0/transitive_lib/src/lib.rs",
            }
        "#]],
        None,
        "sdist-workspace-with-path-dep",
    ))
}

#[test]
fn workspace_with_path_dep_git_sdist_generator() {
    if !Path::new(".git").exists() {
        return;
    }
    handle_result(other::test_source_distribution(
        "test-crates/workspace_with_path_dep/python",
        SdistGenerator::Git,
        expect![[r#"
            {
                "workspace_with_path_dep-0.1.0/Cargo.toml",
                "workspace_with_path_dep-0.1.0/PKG-INFO",
                "workspace_with_path_dep-0.1.0/pyproject.toml",
                "workspace_with_path_dep-0.1.0/src/lib.rs",
            }
        "#]],
        None,
        "sdist-workspace-with-path-dep-git",
    ))
}

#[rustversion::since(1.64)]
#[test]
fn workspace_inheritance_sdist() {
    handle_result(other::test_source_distribution(
        "test-crates/workspace-inheritance/python",
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "workspace_inheritance-0.1.0/Cargo.lock",
                "workspace_inheritance-0.1.0/Cargo.toml",
                "workspace_inheritance-0.1.0/PKG-INFO",
                "workspace_inheritance-0.1.0/generic_lib/Cargo.toml",
                "workspace_inheritance-0.1.0/generic_lib/src/lib.rs",
                "workspace_inheritance-0.1.0/pyproject.toml",
                "workspace_inheritance-0.1.0/python/Cargo.toml",
                "workspace_inheritance-0.1.0/python/src/lib.rs",
            }
        "#]],
        None,
        "sdist-workspace-inheritance",
    ))
}

#[test]
fn workspace_license_files() {
    handle_result(other::test_source_distribution(
        "test-crates/hello-world",
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "hello_world-0.1.0/Cargo.lock",
                "hello_world-0.1.0/Cargo.toml",
                "hello_world-0.1.0/LICENSE",
                "hello_world-0.1.0/PKG-INFO",
                "hello_world-0.1.0/README.md",
                "hello_world-0.1.0/check_installed/check_installed.py",
                "hello_world-0.1.0/licenses/AUTHORS.txt",
                "hello_world-0.1.0/pyproject.toml",
                "hello_world-0.1.0/src/bin/foo.rs",
                "hello_world-0.1.0/src/main.rs",
            }
        "#]],
        None,
        "sdist-hello-world",
    ))
}

#[test]
fn abi3_python_interpreter_args() {
    handle_result(other::abi3_python_interpreter_args());
}

#[test]
fn pyo3_source_date_epoch() {
    env::set_var("SOURCE_DATE_EPOCH", "0");
    handle_result(other::check_wheel_mtimes(
        "test-crates/pyo3-mixed-include-exclude",
        vec![datetime!(1980-01-01 0:00 UTC)],
        "pyo3_source_date_epoch",
    ))
}
