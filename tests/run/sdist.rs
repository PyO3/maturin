use crate::common::{handle_result, other};
use expect_test::expect;
use indoc::indoc;
use maturin::pyproject_toml::SdistGenerator;
use maturin::{BuildOptions, CargoOptions, OutputOptions, unpack_sdist};
use std::path::Path;
use std::process::Command;
use url::Url;
use which::which;

/// A table-driven source distribution scenario.
///
/// The case id is used to derive isolated output paths and failure messages for helper calls.
#[derive(Clone, Copy)]
struct SdistCase<'a> {
    /// Stable identifier used for derived test paths and failure messages.
    id: &'a str,
    /// Repo-relative path to the package under test.
    package: &'a str,
    /// Which sdist generator implementation the case exercises.
    generator: SdistGenerator,
}

fn run_sdist_case(
    case: SdistCase<'_>,
    expected_files: expect_test::Expect,
    expected_cargo_toml: Option<(&Path, expect_test::Expect)>,
) {
    handle_result(other::test_source_distribution(
        case.package,
        case.generator,
        expected_files,
        expected_cargo_toml,
        case.id,
    ));
}

#[test]
fn sdist_excludes_default_run() {
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir.path().join("hello-world");
    other::copy_dir_recursive(Path::new("test-crates/hello-world"), &project_dir).unwrap();

    let cargo_toml_path = project_dir.join("Cargo.toml");
    let mut cargo_toml = fs_err::read_to_string(&cargo_toml_path).unwrap();
    fs_err::write(project_dir.join("README.md"), "Pyproject readme").unwrap();
    let parent_readme = temp_dir.path().join("README.md");
    fs_err::write(&parent_readme, "Cargo readme").unwrap();
    cargo_toml = cargo_toml.replace("../../README.md", "../README.md");
    cargo_toml.push_str("\n[[bin]]\nname = \"excluded_bin\"\npath = \"src/bin/excluded_bin.rs\"\n");
    let cargo_toml = cargo_toml.replace(
        "default-run = \"hello-world\"",
        "default-run = \"excluded_bin\"",
    );
    fs_err::write(&cargo_toml_path, cargo_toml).unwrap();
    fs_err::write(project_dir.join("src/bin/excluded_bin.rs"), "fn main() {}").unwrap();

    let pyproject_toml_path = project_dir.join("pyproject.toml");
    let mut pyproject_toml = fs_err::read_to_string(&pyproject_toml_path).unwrap();
    pyproject_toml =
        pyproject_toml.replace("exclude = [", "exclude = [\n  \"src/bin/excluded_bin.rs\",");
    pyproject_toml = pyproject_toml.replace(
        "dynamic = [\"authors\", \"readme\"]",
        "readme = \"README.md\"\ndynamic = [\"authors\"]",
    );
    fs_err::write(&pyproject_toml_path, pyproject_toml).unwrap();

    let expected_cargo_toml = expect![[r#"
        [package]
        name = "hello-world"
        version = "0.1.0"
        authors = ["konstin <konstin@mailbox.org>"]
        edition = "2021"
        # Test references to out-of-project files
        readme = "README.md"

        [dependencies]

        [[bench]]
        name = "included_bench"

        [[example]]
        name = "included_example"
    "#]];

    handle_result(other::test_source_distribution(
        &project_dir,
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "hello_world-0.1.0/Cargo.lock",
                "hello_world-0.1.0/Cargo.toml",
                "hello_world-0.1.0/LICENSE",
                "hello_world-0.1.0/PKG-INFO",
                "hello_world-0.1.0/README.md",
                "hello_world-0.1.0/benches/included_bench.rs",
                "hello_world-0.1.0/check_installed/check_installed.py",
                "hello_world-0.1.0/examples/included_example.rs",
                "hello_world-0.1.0/licenses/AUTHORS.txt",
                "hello_world-0.1.0/pyproject.toml",
                "hello_world-0.1.0/src/bin/foo.rs",
                "hello_world-0.1.0/src/main.rs",
            }
        "#]],
        Some((
            Path::new("hello_world-0.1.0/Cargo.toml"),
            expected_cargo_toml,
        )),
        "sdist-hello-world-default-run",
    ))
}

#[test]
fn sdist_excludes_implicit_default_run() {
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir.path().join("hello-world");
    other::copy_dir_recursive(Path::new("test-crates/hello-world"), &project_dir).unwrap();

    let cargo_toml_path = project_dir.join("Cargo.toml");
    let cargo_toml = fs_err::read_to_string(&cargo_toml_path)
        .unwrap()
        .replace("../../README.md", "../README.md");
    fs_err::write(&cargo_toml_path, cargo_toml).unwrap();
    fs_err::write(temp_dir.path().join("README.md"), "Cargo readme").unwrap();

    let pyproject_toml_path = project_dir.join("pyproject.toml");
    let pyproject_toml = fs_err::read_to_string(&pyproject_toml_path)
        .unwrap()
        .replace("exclude = [", "exclude = [\n  \"src/main.rs\",");
    fs_err::write(&pyproject_toml_path, pyproject_toml).unwrap();

    let expected_cargo_toml = expect![[r#"
        [package]
        name = "hello-world"
        version = "0.1.0"
        authors = ["konstin <konstin@mailbox.org>"]
        edition = "2021"
        # Test references to out-of-project files
        readme = "README.md"

        [dependencies]

        [[bench]]
        name = "included_bench"

        [[example]]
        name = "included_example"
    "#]];

    handle_result(other::test_source_distribution(
        &project_dir,
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "hello_world-0.1.0/Cargo.lock",
                "hello_world-0.1.0/Cargo.toml",
                "hello_world-0.1.0/LICENSE",
                "hello_world-0.1.0/PKG-INFO",
                "hello_world-0.1.0/README.md",
                "hello_world-0.1.0/benches/included_bench.rs",
                "hello_world-0.1.0/check_installed/check_installed.py",
                "hello_world-0.1.0/examples/included_example.rs",
                "hello_world-0.1.0/licenses/AUTHORS.txt",
                "hello_world-0.1.0/pyproject.toml",
                "hello_world-0.1.0/src/bin/foo.rs",
            }
        "#]],
        Some((
            Path::new("hello_world-0.1.0/Cargo.toml"),
            expected_cargo_toml,
        )),
        "sdist-hello-world-implicit-default-run",
    ))
}

#[test]
fn sdist_excludes_explicit_build_script() {
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir.path().join("buildrs-repro");
    fs_err::create_dir_all(project_dir.join("src")).unwrap();
    fs_err::write(project_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs_err::write(project_dir.join("build.rs"), "fn main() {}\n").unwrap();
    fs_err::write(
        project_dir.join("Cargo.toml"),
        indoc!(
            r#"
            [package]
            name = "buildrs-repro"
            version = "0.1.0"
            edition = "2021"
            build = "build.rs"

            [[bin]]
            name = "buildrs-repro"
            path = "src/main.rs"
            "#
        ),
    )
    .unwrap();
    fs_err::write(
        project_dir.join("pyproject.toml"),
        indoc!(
            r#"
            [project]
            name = "buildrs-repro"
            version = "0.1.0"

            [build-system]
            requires = ["maturin>=1.0,<2.0"]
            build-backend = "maturin"

            [tool.maturin]
            bindings = "bin"
            exclude = ["build.rs"]
            "#
        ),
    )
    .unwrap();

    let sdist_dir = temp_dir.path().join("dist");
    let build_options = BuildOptions {
        output: OutputOptions {
            out: Some(sdist_dir),
            ..Default::default()
        },
        cargo: CargoOptions {
            manifest_path: Some(project_dir.join("Cargo.toml")),
            quiet: true,
            target_dir: Some(temp_dir.path().join("target")),
            ..Default::default()
        },
        ..Default::default()
    };
    let build_context = build_options
        .into_build_context()
        .strip(Some(false))
        .editable(false)
        .sdist_only(true)
        .build()
        .unwrap();
    let (sdist_path, _) = build_context
        .build_source_distribution()
        .unwrap()
        .expect("failed to build sdist");

    let maturin::UnpackedSdist {
        tmpdir: _tmp,
        cargo_toml,
        pyproject_toml: _pyproject_toml,
    } = unpack_sdist(&sdist_path).unwrap();
    let sdist_root = cargo_toml.parent().unwrap();
    assert!(
        !sdist_root.join("build.rs").exists(),
        "build.rs should not be packaged when excluded"
    );
    let rewritten_manifest = fs_err::read_to_string(&cargo_toml).unwrap();
    assert!(
        !rewritten_manifest.contains("build = \"build.rs\""),
        "expected explicit build script to be removed, got:\n{rewritten_manifest}"
    );

    let output = Command::new("cargo")
        .args(["metadata", "--manifest-path"])
        .arg(&cargo_toml)
        .args(["--format-version", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo metadata failed for unpacked sdist\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn workspace_cargo_lock() {
    handle_result(other::test_workspace_cargo_lock())
}

#[test]
fn build_wheels_from_sdist_hello_world() {
    handle_result(other::test_build_wheels_from_sdist(
        "test-crates/hello-world",
        "build_wheels_from_sdist_hello_world",
    ))
}

#[test]
fn workspace_members_beneath_pyproject_sdist() {
    let cargo_toml = expect![[r#"
        [workspace]
        resolver = "2"
        members = ["pyo3-mixed-workspace", "python/pyo3-mixed-workspace-py"]
        "#]];
    run_sdist_case(
        SdistCase {
            id: "sdist-workspace-members-beneath_pyproject",
            package: "test-crates/pyo3-mixed-workspace/rust/python/pyo3-mixed-workspace-py",
            generator: SdistGenerator::Cargo,
        },
        expect![[r#"
            {
                "pyo3_mixed_workspace-2.1.3/PKG-INFO",
                "pyo3_mixed_workspace-2.1.3/README.md",
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
    )
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
        pyo3 = { version = "0.27.0", features = [
            "abi3-py37",
            "generate-import-lib",
        ] }

        [lib]
        name = "pyo3_pure"
        crate-type = ["cdylib"]
    "#]];
    run_sdist_case(
        SdistCase {
            id: "sdist-workspace-members-non-local-dep",
            package: "test-crates/pyo3-pure",
            generator: SdistGenerator::Cargo,
        },
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
    )
}

#[test]
fn lib_with_path_dep_sdist() {
    run_sdist_case(
        SdistCase {
            id: "sdist-lib-with-path-dep",
            package: "test-crates/sdist_with_path_dep",
            generator: SdistGenerator::Cargo,
        },
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
    )
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
        pyo3 = "0.27.0"

        [target.'cfg(not(target_endian = "all-over-the-place"))'.dependencies]
        some_path_dep = { path = "../some_path_dep" }
    "#]];
    run_sdist_case(
        SdistCase {
            id: "sdist-lib-with-target-path-dep",
            package: "test-crates/sdist_with_target_path_dep",
            generator: SdistGenerator::Cargo,
        },
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
    )
}

// Remaining bespoke sdist regressions stay imperative because they synthesize
// custom workspaces and validate hand-written manifest rewrites.
// The body of these tests is moved mechanically from the previous monolithic run.rs.

#[test]
fn lib_with_parent_workspace_path_dep_sdist() {
    let expected_shared_crate_cargo_toml = expect![[r#"
        [package]
        name = "shared_crate"
        version = "0.1.0"
        edition = "2021"
        readme = "README.md"
        publish = false
        include = ["src/**", "README.md", "Cargo.toml"]

        [lib]

        [dev-dependencies]
        log = "^0.4"
    "#]];
    run_sdist_case(
        SdistCase {
            id: "sdist-lib-with-parent-workspace-path-dep",
            package: "test-crates/parent_workspace_sdist/crates/pysof",
            generator: SdistGenerator::Cargo,
        },
        expect![[r#"
            {
                "pysof-0.1.0/PKG-INFO",
                "pysof-0.1.0/pyproject.toml",
                "pysof-0.1.0/pysof/.gitignore",
                "pysof-0.1.0/pysof/Cargo.lock",
                "pysof-0.1.0/pysof/Cargo.toml",
                "pysof-0.1.0/pysof/src/lib.rs",
                "pysof-0.1.0/shared_crate/Cargo.toml",
                "pysof-0.1.0/shared_crate/README.md",
                "pysof-0.1.0/shared_crate/src/lib.rs",
            }
        "#]],
        Some((
            Path::new("pysof-0.1.0/shared_crate/Cargo.toml"),
            expected_shared_crate_cargo_toml,
        )),
    )
}

// ---- BEGIN mechanically moved imperative regressions ----
#[test]
fn lib_with_parent_workspace_git_dep_sdist() {
    if which("git").is_err() {
        eprintln!("Skipping lib_with_parent_workspace_git_dep_sdist: git not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let git_dep_dir = temp_dir.path().join("gitdep");
    fs_err::create_dir_all(git_dep_dir.join("src")).unwrap();
    fs_err::write(
        git_dep_dir.join("Cargo.toml"),
        indoc!(
            r#"
            [package]
            name = "gitdep"
            version = "0.1.0"
            edition = "2021"

            [lib]
            "#
        ),
    )
    .unwrap();
    fs_err::write(git_dep_dir.join("src/lib.rs"), "pub fn from_git() {}\n").unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--initial-branch=main", "-q"])
            .current_dir(&git_dep_dir)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&git_dep_dir)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-c",
                "user.name=maturin-tests",
                "-c",
                "user.email=maturin-tests@example.com",
                "commit",
                "-q",
                "-m",
                "init",
            ])
            .current_dir(&git_dep_dir)
            .status()
            .unwrap()
            .success()
    );

    let git_dep_url = Url::from_directory_path(&git_dep_dir)
        .expect("git dependency path should convert to file:// URL")
        .to_string();

    let workspace_root = temp_dir.path().join("workspace");
    let pysof_dir = workspace_root.join("crates/pysof");
    let shared_dir = workspace_root.join("crates/shared_crate");
    fs_err::create_dir_all(pysof_dir.join("src")).unwrap();
    fs_err::create_dir_all(shared_dir.join("src")).unwrap();
    fs_err::write(workspace_root.join("README.md"), "workspace readme\n").unwrap();
    fs_err::write(
        workspace_root.join("Cargo.toml"),
        format!(
            indoc!(
                r#"
                [workspace]
                members = ["crates/shared_crate"]
                exclude = ["crates/pysof"]
                resolver = "2"

                [workspace.package]
                edition = "2021"
                readme = "README.md"

                [workspace.dependencies]
                gitdep = {{ git = "{}", branch = "main" }}
                "#
            ),
            git_dep_url
        ),
    )
    .unwrap();
    fs_err::write(
        shared_dir.join("Cargo.toml"),
        indoc!(
            r#"
            [package]
            name = "shared_crate"
            version = "0.1.0"
            edition.workspace = true
            readme.workspace = true

            [lib]

            [dependencies]
            gitdep = { workspace = true }
            "#
        ),
    )
    .unwrap();
    fs_err::write(
        shared_dir.join("src/lib.rs"),
        "pub fn use_git() { gitdep::from_git(); }\n",
    )
    .unwrap();
    fs_err::write(
        pysof_dir.join("Cargo.toml"),
        indoc!(
            r#"
            [package]
            name = "pysof"
            version = "0.1.0"
            edition = "2021"

            [lib]
            crate-type = ["cdylib"]

            [dependencies]
            pyo3 = { version = "0.27.0", features = ["extension-module"] }
            shared_crate = { path = "../shared_crate" }
            "#
        ),
    )
    .unwrap();
    fs_err::write(
        pysof_dir.join("src/lib.rs"),
        indoc!(
            r#"
            use pyo3::prelude::*;

            #[pymodule]
            fn pysof(_m: &Bound<'_, PyModule>) -> PyResult<()> {
                shared_crate::use_git();
                Ok(())
            }
            "#
        ),
    )
    .unwrap();
    fs_err::write(
        pysof_dir.join("pyproject.toml"),
        indoc!(
            r#"
            [build-system]
            requires = ["maturin>=1.0,<2.0"]
            build-backend = "maturin"

            [project]
            name = "pysof"
            version = "0.1.0"
            "#
        ),
    )
    .unwrap();

    let sdist_dir = temp_dir.path().join("dist");
    let build_options = BuildOptions {
        output: OutputOptions {
            out: Some(sdist_dir),
            ..Default::default()
        },
        cargo: CargoOptions {
            manifest_path: Some(pysof_dir.join("Cargo.toml")),
            quiet: true,
            target_dir: Some(temp_dir.path().join("target")),
            ..Default::default()
        },
        ..Default::default()
    };
    let build_context = build_options
        .into_build_context()
        .strip(Some(false))
        .editable(false)
        .sdist_only(true)
        .build()
        .unwrap();
    let (sdist_path, _) = build_context
        .build_source_distribution()
        .unwrap()
        .expect("failed to build sdist");

    let maturin::UnpackedSdist {
        tmpdir: _tmp,
        cargo_toml,
        pyproject_toml: _pyproject_toml,
    } = unpack_sdist(&sdist_path).unwrap();
    let sdist_root = cargo_toml.parent().unwrap().parent().unwrap();
    let shared_manifest = sdist_root.join("shared_crate/Cargo.toml");
    let rewritten_shared_manifest = fs_err::read_to_string(&shared_manifest).unwrap();
    assert!(rewritten_shared_manifest.contains("git = \"file://"));
    assert!(rewritten_shared_manifest.contains("branch = \"main\""));

    let output = Command::new("cargo")
        .args(["metadata", "--manifest-path"])
        .arg(&cargo_toml)
        .args(["--format-version", "1"])
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn lib_with_parent_workspace_lints_sdist() {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_root = temp_dir.path().join("workspace");
    let pyapp_dir = workspace_root.join("crates/pyapp");
    let shared_dir = workspace_root.join("crates/shared_crate");
    fs_err::create_dir_all(pyapp_dir.join("src")).unwrap();
    fs_err::create_dir_all(shared_dir.join("src")).unwrap();
    fs_err::write(workspace_root.join("README.md"), "workspace readme\n").unwrap();
    fs_err::write(
        workspace_root.join("Cargo.toml"),
        indoc!(
            r#"
            [workspace]
            members = ["crates/shared_crate"]
            exclude = ["crates/pyapp"]
            resolver = "2"

            [workspace.package]
            edition = "2021"
            readme = "README.md"

            [workspace.lints.rust]
            unsafe_code = "forbid"
            "#
        ),
    )
    .unwrap();
    fs_err::write(
        shared_dir.join("Cargo.toml"),
        indoc!(
            r#"
            [package]
            name = "shared_crate"
            version = "0.1.0"
            edition.workspace = true
            readme.workspace = true

            [lib]

            [lints]
            workspace = true
            "#
        ),
    )
    .unwrap();
    fs_err::write(shared_dir.join("src/lib.rs"), "pub fn hello() {}\n").unwrap();
    fs_err::write(
        pyapp_dir.join("Cargo.toml"),
        indoc!(
            r#"
            [package]
            name = "pyapp"
            version = "0.1.0"
            edition = "2021"

            [lib]
            crate-type = ["cdylib"]

            [dependencies]
            pyo3 = { version = "0.27.0", features = ["extension-module"] }
            shared_crate = { path = "../shared_crate" }
            "#
        ),
    )
    .unwrap();
    fs_err::write(
        pyapp_dir.join("src/lib.rs"),
        indoc!(
            r#"
            use pyo3::prelude::*;

            #[pymodule]
            fn pyapp(_m: &Bound<'_, PyModule>) -> PyResult<()> {
                shared_crate::hello();
                Ok(())
            }
            "#
        ),
    )
    .unwrap();
    fs_err::write(
        pyapp_dir.join("pyproject.toml"),
        indoc!(
            r#"
            [build-system]
            requires = ["maturin>=1.0,<2.0"]
            build-backend = "maturin"

            [project]
            name = "pyapp"
            version = "0.1.0"
            "#
        ),
    )
    .unwrap();

    let sdist_dir = temp_dir.path().join("dist");
    let build_options = BuildOptions {
        output: OutputOptions {
            out: Some(sdist_dir),
            ..Default::default()
        },
        cargo: CargoOptions {
            manifest_path: Some(pyapp_dir.join("Cargo.toml")),
            quiet: true,
            target_dir: Some(temp_dir.path().join("target")),
            ..Default::default()
        },
        ..Default::default()
    };
    let build_context = build_options
        .into_build_context()
        .strip(Some(false))
        .editable(false)
        .sdist_only(true)
        .build()
        .unwrap();
    let (sdist_path, _) = build_context
        .build_source_distribution()
        .unwrap()
        .expect("failed to build sdist");

    let maturin::UnpackedSdist {
        tmpdir: _tmp,
        cargo_toml,
        pyproject_toml: _pyproject_toml,
    } = unpack_sdist(&sdist_path).unwrap();
    let sdist_root = cargo_toml.parent().unwrap().parent().unwrap();
    let shared_manifest = sdist_root.join("shared_crate/Cargo.toml");
    let rewritten_shared_manifest = fs_err::read_to_string(&shared_manifest).unwrap();
    assert!(rewritten_shared_manifest.contains("[lints.rust]"));
    assert!(rewritten_shared_manifest.contains("unsafe_code = \"forbid\""));
    assert!(!rewritten_shared_manifest.contains("workspace = true"));

    let output = Command::new("cargo")
        .args(["metadata", "--manifest-path"])
        .arg(&cargo_toml)
        .args(["--format-version", "1"])
        .output()
        .unwrap();
    assert!(output.status.success());
}
// ---- END mechanically moved imperative regressions ----

#[test]
fn external_python_source_sdist() {
    let pyproject_toml = expect![[r#"
        [project]
        name = "external-python-source"
        version = "0.1.0"

        [build-system]
        requires = ["maturin>=1.0,<2.0"]
        build-backend = "maturin"

        [tool.maturin]
        bindings = "bin"
        module-name = "external_python_source"
        manifest-path = "crate/Cargo.toml"
        python-source = "python"
    "#]];
    run_sdist_case(
        SdistCase {
            id: "sdist-external-python-source",
            package: "test-crates/external-python-source/crate",
            generator: SdistGenerator::Cargo,
        },
        expect![[r#"
            {
                "external_python_source-0.1.0/PKG-INFO",
                "external_python_source-0.1.0/crate/.gitignore",
                "external_python_source-0.1.0/crate/Cargo.lock",
                "external_python_source-0.1.0/crate/Cargo.toml",
                "external_python_source-0.1.0/crate/src/main.rs",
                "external_python_source-0.1.0/pyproject.toml",
                "external_python_source-0.1.0/python/external_python_source/__init__.py",
            }
        "#]],
        Some((
            Path::new("external_python_source-0.1.0/pyproject.toml"),
            pyproject_toml,
        )),
    )
}

#[test]
fn pyo3_mixed_src_layout_sdist() {
    run_sdist_case(
        SdistCase {
            id: "sdist-pyo3-mixed-src-layout",
            package: "test-crates/pyo3-mixed-src/rust",
            generator: SdistGenerator::Cargo,
        },
        expect![[r#"
            {
                "pyo3_mixed_src-2.1.3/PKG-INFO",
                "pyo3_mixed_src-2.1.3/README.md",
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
    )
}

#[test]
fn pyo3_mixed_include_exclude_sdist() {
    run_sdist_case(
        SdistCase {
            id: "sdist-pyo3-mixed-include-exclude",
            package: "test-crates/pyo3-mixed-include-exclude",
            generator: SdistGenerator::Cargo,
        },
        expect![[r#"
            {
                "pyo3_mixed_include_exclude-2.1.3/.gitignore",
                "pyo3_mixed_include_exclude-2.1.3/Cargo.lock",
                "pyo3_mixed_include_exclude-2.1.3/Cargo.toml",
                "pyo3_mixed_include_exclude-2.1.3/PKG-INFO",
                "pyo3_mixed_include_exclude-2.1.3/README.md",
                "pyo3_mixed_include_exclude-2.1.3/build.rs",
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
    )
}

#[test]
fn pyo3_mixed_include_exclude_git_sdist_generator() {
    if !Path::new(".git").exists() {
        return;
    }
    run_sdist_case(
        SdistCase {
            id: "sdist-pyo3-mixed-include-exclude-git",
            package: "test-crates/pyo3-mixed-include-exclude",
            generator: SdistGenerator::Git,
        },
        expect![[r#"
            {
                "pyo3_mixed_include_exclude-2.1.3/.gitignore",
                "pyo3_mixed_include_exclude-2.1.3/Cargo.lock",
                "pyo3_mixed_include_exclude-2.1.3/Cargo.toml",
                "pyo3_mixed_include_exclude-2.1.3/PKG-INFO",
                "pyo3_mixed_include_exclude-2.1.3/README.md",
                "pyo3_mixed_include_exclude-2.1.3/build.rs",
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
    )
}

#[test]
fn workspace_sdist() {
    run_sdist_case(
        SdistCase {
            id: "sdist-workspace",
            package: "test-crates/workspace/py",
            generator: SdistGenerator::Cargo,
        },
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
    )
}

#[test]
fn workspace_with_path_dep_sdist() {
    run_sdist_case(
        SdistCase {
            id: "sdist-workspace-with-path-dep",
            package: "test-crates/workspace_with_path_dep/python",
            generator: SdistGenerator::Cargo,
        },
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
    )
}

#[test]
fn workspace_with_path_dep_git_sdist_generator() {
    if !Path::new(".git").exists() {
        return;
    }
    run_sdist_case(
        SdistCase {
            id: "sdist-workspace-with-path-dep-git",
            package: "test-crates/workspace_with_path_dep/python",
            generator: SdistGenerator::Git,
        },
        expect![[r#"
            {
                "workspace_with_path_dep-0.1.0/Cargo.toml",
                "workspace_with_path_dep-0.1.0/PKG-INFO",
                "workspace_with_path_dep-0.1.0/pyproject.toml",
                "workspace_with_path_dep-0.1.0/src/lib.rs",
            }
        "#]],
        None,
    )
}

#[rustversion::since(1.64)]
#[test]
fn workspace_inheritance_sdist() {
    run_sdist_case(
        SdistCase {
            id: "sdist-workspace-inheritance",
            package: "test-crates/workspace-inheritance/python",
            generator: SdistGenerator::Cargo,
        },
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
    )
}

#[test]
fn workspace_license_files() {
    let cargo_toml = expect![[r#"
        [package]
        name = "hello-world"
        version = "0.1.0"
        authors = ["konstin <konstin@mailbox.org>"]
        edition = "2021"
        # Test references to out-of-project files
        readme = "README.md"
        default-run = "hello-world"

        [dependencies]

        [[bench]]
        name = "included_bench"

        [[example]]
        name = "included_example"
    "#]];
    run_sdist_case(
        SdistCase {
            id: "sdist-hello-world",
            package: "test-crates/hello-world",
            generator: SdistGenerator::Cargo,
        },
        expect![[r#"
            {
                "hello_world-0.1.0/Cargo.lock",
                "hello_world-0.1.0/Cargo.toml",
                "hello_world-0.1.0/LICENSE",
                "hello_world-0.1.0/PKG-INFO",
                "hello_world-0.1.0/README.md",
                "hello_world-0.1.0/benches/included_bench.rs",
                "hello_world-0.1.0/check_installed/check_installed.py",
                "hello_world-0.1.0/examples/included_example.rs",
                "hello_world-0.1.0/licenses/AUTHORS.txt",
                "hello_world-0.1.0/pyproject.toml",
                "hello_world-0.1.0/src/bin/foo.rs",
                "hello_world-0.1.0/src/main.rs",
            }
        "#]],
        Some((Path::new("hello_world-0.1.0/Cargo.toml"), cargo_toml)),
    )
}
