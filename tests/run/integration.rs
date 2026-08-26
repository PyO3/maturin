use crate::common::integration::{self, IntegrationCase};
use crate::common::other;
use crate::common::{
    CFFI_MIXED_IMPLICIT_COPY, CFFI_MIXED_INCLUDE_EXCLUDE_COPY, CFFI_MIXED_PY_SUBDIR_COPY,
    CFFI_MIXED_SRC_COPY, CFFI_MIXED_SUBMODULE_COPY, CFFI_MIXED_WITH_PATH_DEP_COPY, handle_result,
    has_conda, has_uniffi_bindgen, has_uv, is_ci, test_python_implementation,
    test_python_supports_abi3t,
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
#[case::pyo3_stub_generation_pure(IntegrationCase::new(
    "integration-pyo3-stub-generation-pure",
    "test-crates/pyo3-stub-generation-pure",
).generate_stubs())]
#[cfg_attr(unix, case::pyo3_stub_generation_pure_zig(IntegrationCase::new(
    "integration-pyo3-stub-generation-pure-zig",
    "test-crates/pyo3-stub-generation-pure",
).generate_stubs().zig()))]
#[case::pyo3_stub_generation_mixed(IntegrationCase::new(
    "integration-pyo3-stub-generation-mixed",
    "test-crates/pyo3-stub-generation-mixed",
).generate_stubs())]
#[case::pyo3_stub_generation_mixed_py_subdir(IntegrationCase::new(
    "integration-pyo3-stub-generation-mixed-py-subdir",
    "test-crates/pyo3-stub-generation-mixed-py-subdir",
).generate_stubs())]
#[test]
fn integration_cases(#[case] case: IntegrationCase<'_>) {
    handle_result(integration::test_integration(&case));
}

#[test]
fn integration_pyo3_bin_uv_multi_python() {
    if has_uv() {
        handle_result(integration::test_integration_uv_multi_python(
            &IntegrationCase::new(
                "integration-pyo3-bin-uv-multi-python",
                "test-crates/pyo3-bin",
            ),
        ));
    }
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
fn integration_wasm_host_only_dependency() {
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir.path().join("hello-world");
    other::copy_dir_recursive(Path::new("test-crates/hello-world"), &project_dir).unwrap();

    let manifest_path = project_dir.join("Cargo.toml");
    let mut manifest = fs_err::read_to_string(&manifest_path)
        .unwrap()
        .replace("../../README.md", "../README.md");
    manifest.push_str("\n[build-dependencies]\nhost-helper = { path = \"../host-helper\" }\n");
    fs_err::write(&manifest_path, manifest).unwrap();
    fs_err::write(temp_dir.path().join("README.md"), "Cargo readme").unwrap();
    fs_err::write(
        project_dir.join("build.rs"),
        "fn main() { host_helper::run(); }\n",
    )
    .unwrap();

    // The build script runs on the host, so Cargo compiles host-only even though
    // metadata filtered for the WASI target excludes it.
    for (name, dependencies, source) in [
        (
            "host-helper",
            indoc::indoc! {r#"
                [target.'cfg(not(target_family = "wasm"))'.dependencies]
                host-only = { path = "../host-only" }
            "#},
            "pub fn run() { host_only::run(); }\n",
        ),
        ("host-only", "", "pub fn run() {}\n"),
    ] {
        let dependency_dir = temp_dir.path().join(name);
        fs_err::create_dir_all(dependency_dir.join("src")).unwrap();
        fs_err::write(
            dependency_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = {name:?}\nversion = \"0.1.0\"\nedition = \"2021\"\n{dependencies}"
            ),
        )
        .unwrap();
        fs_err::write(dependency_dir.join("src/lib.rs"), source).unwrap();
    }

    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&manifest_path)
        .other_options(vec!["--filter-platform".into(), "wasm32-wasip1".into()])
        .exec()
        .unwrap();
    assert!(metadata.packages.iter().all(|pkg| pkg.name != "host-only"));

    let wheel_dir = temp_dir.path().join("wheels");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_maturin"))
        .args(["build", "--target", "wasm32-wasip1", "--offline"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("--target-dir")
        .arg(temp_dir.path().join("target"))
        .arg("--out")
        .arg(&wheel_dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        !stderr.contains("wasn't listed in `cargo metadata`"),
        "{stderr}"
    );

    let wheels = fs_err::read_dir(&wheel_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(wheels.len(), 1);
    let mut wheel = zip::ZipArchive::new(fs_err::File::open(&wheels[0]).unwrap()).unwrap();
    for binary in ["hello-world", "foo"] {
        assert!(
            wheel
                .by_name(&format!("hello_world-0.1.0.data/scripts/{binary}.wasm"))
                .is_ok(),
            "missing {binary} in wheel"
        );
    }
}

#[test]
fn abi3_without_version() {
    handle_result(other::abi3_without_version())
}

#[test]
fn pyo3_cffi_build_script() {
    handle_result(other::pyo3_cffi_build_script())
}

#[test]
fn abi3t_without_version() {
    // abi3t requires CPython >= 3.15 (PEP 803). On older runners the build
    // would reject the only available interpreter, so skip cleanly.
    if !test_python_supports_abi3t() {
        return;
    }
    handle_result(other::abi3t_without_version())
}

#[test]
fn abi3_and_abi3t_wheel_selection() {
    handle_result(other::combined_stable_abi_wheel_selection(
        "abi3-and-abi3t-wheel-selection",
        &[],
    ));
}

#[test]
fn abi3_and_current_abi3t_wheel_selection() {
    handle_result(other::combined_stable_abi_wheel_selection(
        "abi3-and-current-abi3t-wheel-selection",
        &["abi3-and-current-abi3t"],
    ));
}

#[test]
fn integration_pyo3_abi3t() {
    // abi3t requires CPython >= 3.15 (PEP 803).
    if !test_python_supports_abi3t() {
        return;
    }
    handle_result(integration::test_integration(&IntegrationCase::new(
        "integration-pyo3-abi3t",
        "test-crates/pyo3-abi3t",
    )));
}

#[test]
fn abi3_python_interpreter_args() {
    handle_result(other::abi3_python_interpreter_args());
}

#[test]
fn abi3_generate_stubs() {
    handle_result(other::generate_stubs(
        "test-crates/pyo3-stub-generation-pure",
        &[
            "pyo3_stub_generation_pure/__init__.pyi",
            "pyo3_stub_generation_pure/submodule.pyi",
        ],
    ));
}

/// `module-name` places the extension inside a python package, and the stubs have to follow it
/// there — writing `_pyo3_mixed.pyi` to the root of `--out` puts it where nothing will look for it.
#[test]
fn abi3_generate_stubs_mixed_py_subdir() {
    handle_result(other::generate_stubs(
        "test-crates/pyo3-stub-generation-mixed-py-subdir",
        &["pyo3_stub_generation_mixed_py_subdir/_pyo3_mixed.pyi"],
    ));
}
