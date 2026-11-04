use crate::common::{handle_result, other};
use expect_test::{Expect, expect};
use indoc::indoc;
use maturin::pyproject_toml::SdistGenerator;
use maturin::{BuildOptions, BuildOrchestrator, CargoOptions, OutputOptions, unpack_sdist};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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
    let sdist = BuildOrchestrator::new(&build_context)
        .build_source_distribution()
        .unwrap()
        .expect("failed to build sdist");

    let maturin::UnpackedSdist {
        tmpdir: _tmp,
        cargo_toml,
        pyproject_toml: _pyproject_toml,
    } = unpack_sdist(&sdist.path).unwrap();
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

fn write_workspace_bin_project(
    workspace_dir: &Path,
    member: &str,
    project_metadata: &str,
) -> PathBuf {
    fs_err::write(
        workspace_dir.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [\"{member}\"]\n"),
    )
    .unwrap();

    let project_dir = workspace_dir.join(member);
    fs_err::create_dir_all(project_dir.join("src")).unwrap();
    fs_err::write(project_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs_err::write(
        project_dir.join("Cargo.toml"),
        indoc!(
            r#"
            [package]
            name = "python-pkg"
            version = "0.1.0"
            edition = "2021"

            [[bin]]
            name = "python-pkg"
            path = "src/main.rs"
            "#
        ),
    )
    .unwrap();
    fs_err::write(
        project_dir.join("pyproject.toml"),
        format!(
            indoc!(
                r#"
                [project]
                name = "python-pkg"
                version = "0.1.0"

                {}

                [build-system]
                requires = ["maturin>=1.0,<2.0"]
                build-backend = "maturin"

                [tool.maturin]
                bindings = "bin"
                "#
            ),
            project_metadata.trim_end()
        ),
    )
    .unwrap();
    project_dir
}

#[test]
fn sdist_root_dependency_outside_default_members() {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path();
    let project = write_workspace_bin_project(workspace, "py", "");
    fs_err::write(
        workspace.join("Cargo.toml"),
        indoc! {r#"
            [workspace]
            resolver = "2"
            members = ["py"]
            default-members = ["py"]

            [package]
            name = "root"
            version = "0.1.0"
            edition = "2021"
        "#},
    )
    .unwrap();
    fs_err::create_dir(workspace.join("src")).unwrap();
    fs_err::write(workspace.join("src/lib.rs"), "mod factory;\n").unwrap();
    fs_err::write(workspace.join("src/factory.rs"), "// Root crate source\n").unwrap();
    let manifest = fs_err::read_to_string(project.join("Cargo.toml")).unwrap();
    fs_err::write(
        project.join("Cargo.toml"),
        format!("{manifest}\n[dependencies]\nroot = {{ path = \"..\" }}\n"),
    )
    .unwrap();

    let mut archive = other::build_source_distribution(
        &project,
        SdistGenerator::Cargo,
        "sdist-root-dependency-outside-default-members",
    )
    .unwrap();
    let extracted = tempfile::tempdir().unwrap();
    archive.unpack(extracted.path()).unwrap();
    let packaged = extracted.path().join("python_pkg-0.1.0");
    for source in ["src/lib.rs", "src/factory.rs", "py/src/main.rs"] {
        assert_eq!(
            fs_err::read(packaged.join(source)).unwrap(),
            fs_err::read(workspace.join(source)).unwrap(),
            "missing or incorrect contents for {source}"
        );
    }
}

/// Regression test for https://github.com/PyO3/maturin/issues/3181 +
/// follow-up: when `pyproject.toml` is in a workspace member subdirectory and
/// references path metadata outside the member directory, those files must be
/// included and the elevated `pyproject.toml` must be rewritten to the files'
/// archive paths.
#[test]
fn sdist_workspace_member_readme_in_parent() {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_dir = temp_dir.path();
    let crate_group_dir = workspace_dir.join("crates");
    fs_err::create_dir_all(&crate_group_dir).unwrap();
    fs_err::write(crate_group_dir.join("README.md"), "# my-pkg\n").unwrap();
    fs_err::write(crate_group_dir.join("LICENSE"), "MIT\n").unwrap();

    let project_dir = write_workspace_bin_project(
        workspace_dir,
        "crates/python-pkg",
        indoc!(
            r#"
            [project.readme]
            file = "../README.md"
            content-type = "text/markdown"

            [project.license]
            file = "../LICENSE"
            "#
        ),
    );

    handle_result(other::test_source_distribution(
        &project_dir,
        SdistGenerator::Cargo,
        expect![[r#"
            {
                "python_pkg-0.1.0/Cargo.lock",
                "python_pkg-0.1.0/Cargo.toml",
                "python_pkg-0.1.0/PKG-INFO",
                "python_pkg-0.1.0/crates/LICENSE",
                "python_pkg-0.1.0/crates/README.md",
                "python_pkg-0.1.0/crates/python-pkg/Cargo.toml",
                "python_pkg-0.1.0/crates/python-pkg/src/main.rs",
                "python_pkg-0.1.0/pyproject.toml",
            }
        "#]],
        Some((
            Path::new("python_pkg-0.1.0/pyproject.toml"),
            expect![[r#"
                [project]
                name = "python-pkg"
                version = "0.1.0"

                [project.readme]
                file = "crates/README.md"
                content-type = "text/markdown"

                [project.license]
                file = "crates/LICENSE"

                [build-system]
                requires = ["maturin>=1.0,<2.0"]
                build-backend = "maturin"

                [tool.maturin]
                bindings = "bin"
                manifest-path = "crates/python-pkg/Cargo.toml"
            "#]],
        )),
        "sdist-workspace-member-readme-in-parent",
    ))
}

/// Build an sdist and check that every `License-File` in `PKG-INFO` exists at
/// that path relative to the sdist root, as PyPI requires.
fn assert_sdist_license_files(project_dir: &Path, unique_name: &str, expected_files: Expect) {
    let mut archive =
        other::build_source_distribution(project_dir, SdistGenerator::Cargo, unique_name).unwrap();
    let mut files = BTreeSet::new();
    let mut pkg_info = String::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().display().to_string();
        if path.ends_with("/PKG-INFO") {
            entry.read_to_string(&mut pkg_info).unwrap();
        }
        files.insert(path);
    }
    expected_files.assert_debug_eq(&files);

    let root_dir = files.first().unwrap().split('/').next().unwrap();
    let license_files: Vec<_> = pkg_info
        .lines()
        .filter_map(|line| line.strip_prefix("License-File: "))
        .collect();
    assert!(!license_files.is_empty(), "no License-File in PKG-INFO");
    for license_file in license_files {
        assert!(
            files.contains(&format!("{root_dir}/{license_file}")),
            "`License-File: {license_file}` is missing from the sdist: {files:?}"
        );
    }
}

/// Regression test for https://github.com/PyO3/maturin/issues/3327: license
/// files that are not listed in `project.license-files` must also be placed at
/// the sdist root when the crate is not at the sdist root.
#[test]
fn sdist_auto_discovered_license_files_at_sdist_root() {
    // A workspace member with `license-files` in `project.dynamic`, like
    // `pyo3-introspection`.
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = write_workspace_bin_project(
        temp_dir.path(),
        "python-pkg",
        r#"dynamic = ["license-files"]"#,
    );
    fs_err::write(project_dir.join("LICENSE-APACHE"), "Apache-2.0\n").unwrap();
    fs_err::write(project_dir.join("LICENSE-MIT"), "MIT\n").unwrap();
    assert_sdist_license_files(
        &project_dir,
        "sdist-workspace-member-auto-license-files",
        expect![[r#"
            {
                "python_pkg-0.1.0/Cargo.lock",
                "python_pkg-0.1.0/Cargo.toml",
                "python_pkg-0.1.0/LICENSE-APACHE",
                "python_pkg-0.1.0/LICENSE-MIT",
                "python_pkg-0.1.0/PKG-INFO",
                "python_pkg-0.1.0/pyproject.toml",
                "python_pkg-0.1.0/python-pkg/Cargo.toml",
                "python_pkg-0.1.0/python-pkg/LICENSE-APACHE",
                "python_pkg-0.1.0/python-pkg/LICENSE-MIT",
                "python_pkg-0.1.0/python-pkg/src/main.rs",
            }
        "#]],
    );

    // `pyproject.toml` next to the license, with the crate in a subdirectory.
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir.path();
    let crate_dir = project_dir.join("rust");
    fs_err::create_dir_all(crate_dir.join("src")).unwrap();
    fs_err::write(crate_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs_err::write(
        crate_dir.join("Cargo.toml"),
        indoc!(
            r#"
            [package]
            name = "python-pkg"
            version = "0.1.0"
            edition = "2021"
            "#
        ),
    )
    .unwrap();
    fs_err::write(
        project_dir.join("pyproject.toml"),
        indoc!(
            r#"
            [project]
            name = "python-pkg"
            version = "0.1.0"

            [build-system]
            requires = ["maturin>=1.0,<2.0"]
            build-backend = "maturin"

            [tool.maturin]
            bindings = "bin"
            manifest-path = "rust/Cargo.toml"
            "#
        ),
    )
    .unwrap();
    fs_err::write(project_dir.join("LICENSE"), "MIT\n").unwrap();
    assert_sdist_license_files(
        &crate_dir,
        "sdist-manifest-path-auto-license-files",
        expect![[r#"
            {
                "python_pkg-0.1.0/LICENSE",
                "python_pkg-0.1.0/PKG-INFO",
                "python_pkg-0.1.0/pyproject.toml",
                "python_pkg-0.1.0/rust/Cargo.lock",
                "python_pkg-0.1.0/rust/Cargo.toml",
                "python_pkg-0.1.0/rust/src/main.rs",
            }
        "#]],
    );
}

#[test]
fn git_sdist_rejects_parent_relative_pyproject_metadata_paths() {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_dir = temp_dir.path();
    fs_err::write(workspace_dir.join("README.md"), "# my-pkg\n").unwrap();
    let project_dir =
        write_workspace_bin_project(workspace_dir, "python-pkg", "readme = \"../README.md\"");

    let err = match other::build_source_distribution(
        &project_dir,
        SdistGenerator::Git,
        "git-sdist-rejects-parent-relative-pyproject-metadata-paths",
    ) {
        Ok(_) => panic!("expected git sdist generator to reject parent-relative readme"),
        Err(err) => err,
    };
    let err = format!("{err:#}");
    assert!(
        err.contains("must not contain `..` when using the git sdist generator"),
        "unexpected error: {err}"
    );
}

#[test]
fn build_wheels_from_sdist_hello_world() {
    handle_result(other::test_build_wheels_from_sdist(
        "test-crates/hello-world",
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
        pyo3 = { version = "0.29.0", features = [
            "abi3-py39",
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
        pyo3 = { version = "0.29.0" }

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
            pyo3 = { version = "0.29.0", features = ["extension-module"] }
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
    let sdist = BuildOrchestrator::new(&build_context)
        .build_source_distribution()
        .unwrap()
        .expect("failed to build sdist");

    let maturin::UnpackedSdist {
        tmpdir: _tmp,
        cargo_toml,
        pyproject_toml: _pyproject_toml,
    } = unpack_sdist(&sdist.path).unwrap();
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
            pyo3 = { version = "0.29.0", features = ["extension-module"] }
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
    let sdist = BuildOrchestrator::new(&build_context)
        .build_source_distribution()
        .unwrap()
        .expect("failed to build sdist");

    let maturin::UnpackedSdist {
        tmpdir: _tmp,
        cargo_toml,
        pyproject_toml: _pyproject_toml,
    } = unpack_sdist(&sdist.path).unwrap();
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
fn target_dir_in_python_package_sdist() {
    handle_result(other::test_target_dir_in_python_package_sdist())
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

#[rstest::rstest]
#[case::virtual_workspace(false, false, false)]
#[case::root_package(true, false, false)]
#[case::root_path_dependency(true, true, false)]
#[case::leftover_member_lockfile(false, false, true)]
fn sdist_workspace_lockfile(
    #[case] root_package: bool,
    #[case] root_dependency: bool,
    #[case] member_lockfile: bool,
    #[values(None, Some("1.74.0"), Some("1.86.0"))] toolchain: Option<&str>,
) {
    // Exercise prebuilt Maturin with Cargo older than its build MSRV. CI
    // installs these toolchains in one Linux job; other jobs test their default.
    if let Some(toolchain) = toolchain
        && !Command::new("rustup")
            .args(["run", toolchain, "cargo", "--version"])
            .output()
            .is_ok_and(|output| output.status.success())
    {
        eprintln!("Skipping sdist_workspace_lockfile: {toolchain} is not installed");
        return;
    }
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path().join("workspace");
    let command = |program: &str| {
        let mut command = Command::new(program);
        command
            .current_dir(workspace.join("py"))
            // Cargo's test harness sets CARGO to its own executable. Let the
            // runtime toolchain select both Cargo and rustc for wheel builds.
            .env_remove("CARGO")
            .env("CARGO_HOME", temp_dir.path().join("cargo-home"));
        if let Some(toolchain) = toolchain {
            command.env("RUSTUP_TOOLCHAIN", toolchain);
        }
        command
    };
    for name in ["py", "shared", "unused", "unused-dep"] {
        let crate_dir = workspace.join(name);
        fs_err::create_dir_all(crate_dir.join("src")).unwrap();
        let dependencies = match name {
            "py" if root_dependency => {
                "shared = { path = \"../shared\" }\nroot = { path = \"..\" }"
            }
            "py" => "shared = { path = \"../shared\" }",
            "unused" => "shared = { path = \"../unused-dep\" }",
            _ => "",
        };
        // Two versions of the same dependency require version-qualified entries
        // in Cargo.lock. Only one version is needed by the packaged workspace.
        let (package_name, version, standalone) = match name {
            "shared" => ("shared", "0.2.0", "\n[workspace]\n"),
            "unused-dep" => ("shared", "0.1.0", "\n[workspace]\n"),
            _ => (name, "0.1.0", ""),
        };
        fs_err::write(
            crate_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"{version}\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}\n{standalone}"
            ),
        )
        .unwrap();
        let (target, source) = if name == "py" {
            ("main.rs", "fn main() {}\n")
        } else {
            ("lib.rs", "")
        };
        fs_err::write(crate_dir.join("src").join(target), source).unwrap();
    }
    let mut manifest = String::from(
        "[workspace]\nresolver = \"2\"\nmembers = [\"py\", \"unused\"]\ndefault-members = [\"unused\"]\nexclude = [\"shared\", \"unused-dep\"]\n",
    );
    if root_package {
        manifest.push_str("\n[package]\nname = \"root\"\nversion = \"0.1.0\"\n");
        fs_err::create_dir(workspace.join("src")).unwrap();
        fs_err::write(workspace.join("src/lib.rs"), "mod factory;\n").unwrap();
        fs_err::write(workspace.join("src/factory.rs"), "").unwrap();
    }
    fs_err::write(workspace.join("Cargo.toml"), manifest).unwrap();
    fs_err::write(
        workspace.join("py/pyproject.toml"),
        indoc! {r#"
            [build-system]
            requires = ["maturin>=1.0,<2.0"]
            build-backend = "maturin"

            [tool.maturin]
            locked = true
        "#},
    )
    .unwrap();
    let status = command("cargo")
        .args(["generate-lockfile", "--offline"])
        .status()
        .unwrap();
    assert!(status.success());
    let original_lockfile = fs_err::read(workspace.join("Cargo.lock")).unwrap();
    if member_lockfile {
        // Cargo ignores lockfiles left in workspace members, even invalid ones.
        fs_err::write(workspace.join("py/Cargo.lock"), "not a lockfile\n").unwrap();
    }

    let dist = temp_dir.path().join("dist");
    let output = command(env!("CARGO_BIN_EXE_maturin"))
        .args(["sdist", "--out"])
        .arg(&dist)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "sdist failed with {toolchain:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let archive = fs_err::File::open(dist.join("py-0.1.0.tar.gz")).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let extracted = temp_dir.path().join("extracted");
    archive.unpack(&extracted).unwrap();
    let packaged = extracted.join("py-0.1.0");
    assert!(packaged.join("pyproject.toml").is_file());
    assert!(!packaged.join("py/Cargo.lock").exists());
    assert!(!packaged.join("unused").exists());
    assert!(!packaged.join("unused-dep").exists());
    assert_eq!(packaged.join("src/factory.rs").exists(), root_dependency);
    assert_eq!(
        fs_err::read(workspace.join("Cargo.lock")).unwrap(),
        original_lockfile,
        "building an sdist must not change the repository lockfile"
    );
    let lockfile: toml::Value =
        toml::from_str(&fs_err::read_to_string(packaged.join("Cargo.lock")).unwrap()).unwrap();
    let original: toml::Value =
        toml::from_str(std::str::from_utf8(&original_lockfile).unwrap()).unwrap();
    assert_eq!(lockfile["version"], original["version"]);
    let packages: Vec<_> = lockfile["package"]
        .as_array()
        .unwrap()
        .iter()
        .map(|package| {
            (
                package["name"].as_str().unwrap(),
                package["version"].as_str().unwrap(),
            )
        })
        .collect();
    let mut expected_packages = vec![("py", "0.1.0")];
    if root_dependency {
        expected_packages.push(("root", "0.1.0"));
    }
    expected_packages.push(("shared", "0.2.0"));
    assert_eq!(packages, expected_packages);

    // Neither vendoring nor building the extracted sdist may rely on files
    // left in the source workspace.
    fs_err::remove_dir_all(&workspace).unwrap();
    let output = command("cargo")
        .args([
            "vendor",
            "--manifest-path",
            "Cargo.toml",
            "--locked",
            "--offline",
            "VENDOR",
        ])
        .current_dir(&packaged)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "locked vendoring failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut build = command(env!("CARGO_BIN_EXE_maturin"));
    build
        .current_dir(&packaged)
        .args(["build", "--bindings", "bin", "--locked", "--offline"]);
    if cfg!(target_os = "linux") {
        build.args(["--compatibility", "linux"]);
    }
    let output = build.output().unwrap();
    assert!(
        output.status.success(),
        "building the extracted sdist failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[rstest::rstest]
#[case::relative(true, "config/overrides.toml", "py/config/overrides.toml", "registry")]
#[case::equals_in_filename(
    true,
    "config/overrides=sdist.toml",
    "py/config/overrides=sdist.toml",
    "registry"
)]
#[case::parent_relative(
    true,
    "../config/overrides.toml",
    "config/overrides.toml",
    "py/registry"
)]
#[case::top_level(false, "config/overrides.toml", "config/overrides.toml", "registry")]
fn sdist_cargo_config_file(
    #[case] workspace_member: bool,
    #[case] config_path: &str,
    #[case] packaged_config_path: &str,
    #[case] registry_path: &str,
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path().join("workspace");
    let project_dir = if workspace_member {
        workspace.join("py")
    } else {
        workspace.clone()
    };
    let mut crates = vec![(project_dir.clone(), "py")];
    if workspace_member {
        crates.push((workspace.join("unused"), "unused"));
    }
    for (path, member) in crates {
        fs_err::create_dir_all(path.join("src")).unwrap();
        fs_err::write(
            path.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\ninclude = [\"src/**\", \"pyproject.toml\", \"registry/**\"]\n"
            ),
        )
        .unwrap();
        fs_err::write(path.join("src/main.rs"), "fn main() {}\n").unwrap();
    }
    fs_err::OpenOptions::new()
        .append(true)
        .open(project_dir.join("Cargo.toml"))
        .unwrap()
        .write_all(b"[dependencies]\nregistry-dep = \"1\"\n")
        .unwrap();
    if workspace_member {
        fs_err::write(
            workspace.join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"py\", \"unused\"]\n",
        )
        .unwrap();
    }

    // Package a real local registry so both the config reference and paths
    // inside the config must remain valid after removing the source workspace.
    let registry = project_dir.join("registry");
    fs_err::create_dir_all(registry.join("index/re/gi")).unwrap();
    let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (path, data) in [
        (
            "Cargo.toml",
            "[package]\nname = \"registry-dep\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub fn check() {}\n"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(
                &mut header,
                format!("registry-dep-1.0.0/{path}"),
                data.as_bytes(),
            )
            .unwrap();
    }
    let archive = archive.into_inner().unwrap().finish().unwrap();
    let checksum = cargo_lock::Checksum::Sha256(Sha256::digest(&archive).into()).to_string();
    fs_err::write(registry.join("registry-dep-1.0.0.crate"), archive).unwrap();
    fs_err::write(
        registry.join("index/re/gi/registry-dep"),
        format!(
            "{}\n",
            serde_json::json!({
                "name": "registry-dep", "vers": "1.0.0", "deps": [],
                "cksum": checksum, "features": {}, "yanked": false,
            })
        ),
    )
    .unwrap();

    let config_file = project_dir.join(config_path);
    fs_err::create_dir_all(config_file.parent().unwrap()).unwrap();
    fs_err::write(
        &config_file,
        format!(
            "[source.crates-io]\nreplace-with = \"test-registry\"\n[source.test-registry]\nlocal-registry = \"{registry_path}\"\n"
        ),
    )
    .unwrap();
    let inline_config = "env.MATURIN_SDIST_INLINE=\"enabled\"";
    fs_err::write(
        project_dir.join("pyproject.toml"),
        format!(
            "[build-system]\nrequires = [\"maturin>=1,<2\"]\nbuild-backend = \"maturin\"\n[tool.maturin]\nbindings = \"bin\"\nlocked = true\nconfig = {}\n",
            serde_json::json!([config_path, inline_config]),
        ),
    )
    .unwrap();
    fs_err::write(
        project_dir.join("src/main.rs"),
        "fn main() { registry_dep::check(); assert_eq!(env!(\"MATURIN_SDIST_INLINE\"), \"enabled\"); }\n",
    )
    .unwrap();
    let run = |command: &mut Command| {
        let output = command.env("CARGO_NET_OFFLINE", "true").output().unwrap();
        assert!(
            output.status.success(),
            "{command:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let cargo_home = temp_dir.path().join("cargo-home");
    run(Command::new("cargo")
        .args(["generate-lockfile", "--config", config_path])
        .current_dir(&project_dir)
        .env("CARGO_HOME", &cargo_home));
    let dist = temp_dir.path().join("dist");
    run(Command::new(env!("CARGO_BIN_EXE_maturin"))
        .args(["sdist", "--out"])
        .arg(&dist)
        .current_dir(&project_dir)
        .env("CARGO_HOME", &cargo_home));
    let archive = fs_err::File::open(dist.join("py-0.1.0.tar.gz")).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let extracted = temp_dir.path().join("extracted");
    archive.unpack(&extracted).unwrap();
    let packaged = extracted.join("py-0.1.0");
    let pyproject: toml::Value =
        toml::from_str(&fs_err::read_to_string(packaged.join("pyproject.toml")).unwrap()).unwrap();
    assert_eq!(
        pyproject["tool"]["maturin"]["config"][0].as_str(),
        Some(packaged_config_path)
    );
    assert_eq!(
        pyproject["tool"]["maturin"]["config"][1].as_str(),
        Some(inline_config)
    );
    assert_eq!(
        fs_err::read(packaged.join(packaged_config_path)).unwrap(),
        fs_err::read(config_file).unwrap()
    );
    fs_err::remove_dir_all(&workspace).unwrap();
    let mut build = Command::new(env!("CARGO_BIN_EXE_maturin"));
    build
        .args(["build", "--locked", "--offline"])
        .current_dir(&packaged)
        .env("CARGO_HOME", temp_dir.path().join("empty-cargo-home"));
    if cfg!(target_os = "linux") {
        build.args(["--compatibility", "linux"]);
    }
    run(&mut build);
}

#[cfg(unix)]
#[rstest::rstest]
fn sdist_cargo_config_symlink_parent(
    #[values(false, true)] workspace_member: bool,
    #[values(false, true)] relative_asset: bool,
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path().join("workspace");
    let project_dir = if workspace_member {
        workspace.join("py")
    } else {
        workspace.clone()
    };
    fs_err::create_dir_all(project_dir.join("src")).unwrap();
    fs_err::write(
        project_dir.join("Cargo.toml"),
        "[package]\nname = \"py\"\nversion = \"0.1.0\"\nedition = \"2021\"\ninclude = [\"src/**\", \"pyproject.toml\", \"alias/marker\"]\n",
    )
    .unwrap();
    if workspace_member {
        fs_err::create_dir_all(workspace.join("unused/src")).unwrap();
        fs_err::write(workspace.join("unused/src/lib.rs"), "").unwrap();
        fs_err::write(
            workspace.join("unused/Cargo.toml"),
            "[package]\nname = \"unused\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs_err::write(
            workspace.join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"py\", \"unused\"]\n",
        )
        .unwrap();
    }
    let shared = temp_dir.path().join("shared");
    fs_err::create_dir_all(shared.join("deep")).unwrap();
    fs_err::os::unix::fs::symlink(shared.join("deep"), project_dir.join("alias")).unwrap();
    let config = "[env]\nMATURIN_CONFIG_CONTENT = \"good\"\nMATURIN_CONFIG_MARKER = { value = \"marker\", relative = true }\n";
    fs_err::write(shared.join("overrides.toml"), config).unwrap();
    // Lexically collapsing alias/.. would copy this different file.
    fs_err::write(
        project_dir.join("overrides.toml"),
        "[env]\nMATURIN_CONFIG_CONTENT = \"bad\"\n",
    )
    .unwrap();
    let asset_check = if relative_asset {
        fs_err::write(shared.join("deep/marker"), "good").unwrap();
        "const _: [(); 4] = [(); include_str!(env!(\"MATURIN_CONFIG_MARKER\")).len()];\n"
    } else {
        ""
    };
    fs_err::write(
        project_dir.join("src/main.rs"),
        format!(
            "const _: [(); 4] = [(); env!(\"MATURIN_CONFIG_CONTENT\").len()];\n{asset_check}fn main() {{}}\n"
        ),
    )
    .unwrap();
    fs_err::write(
        project_dir.join("pyproject.toml"),
        "[build-system]\nrequires = [\"maturin>=1,<2\"]\nbuild-backend = \"maturin\"\n[tool.maturin]\nbindings = \"bin\"\nlocked = true\nconfig = [\"alias/../overrides.toml\"]\n",
    )
    .unwrap();
    let run = |command: &mut Command| {
        let output = command
            .env("CARGO_HOME", temp_dir.path().join("cargo-home"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(Command::new("cargo")
        .args(["build", "--config", "alias/../overrides.toml"])
        .current_dir(&project_dir));
    let dist = temp_dir.path().join("dist");
    run(Command::new(env!("CARGO_BIN_EXE_maturin"))
        .args(["sdist", "--out"])
        .arg(&dist)
        .current_dir(&project_dir));
    let archive = fs_err::File::open(dist.join("py-0.1.0.tar.gz")).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let extracted = temp_dir.path().join("extracted");
    archive.unpack(&extracted).unwrap();
    let packaged = extracted.join("py-0.1.0");
    let packaged_project = if workspace_member {
        packaged.join("py")
    } else {
        packaged.clone()
    };
    assert_eq!(
        fs_err::read_to_string(packaged_project.join("overrides.toml")).unwrap(),
        config
    );
    assert!(packaged_project.join("alias").is_dir());
    fs_err::remove_dir_all(&workspace).unwrap();
    fs_err::remove_dir_all(&shared).unwrap();
    let mut build = Command::new(env!("CARGO_BIN_EXE_maturin"));
    build
        .args(["build", "--locked", "--offline"])
        .current_dir(&packaged);
    if cfg!(target_os = "linux") {
        build.args(["--compatibility", "linux"]);
    }
    run(&mut build);
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

#[rstest::rstest]
#[case::edition_2021("2021", false, "2")]
#[case::edition_2024("2024", false, "3")]
#[case::inherited_edition("2024", true, "3")]
fn sdist_workspace_resolver(
    #[case] edition: &str,
    #[case] inherited: bool,
    #[case] resolver: &str,
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path().join("workspace");
    for member in ["", "py", "shared"] {
        fs_err::create_dir_all(workspace.join(member).join("src")).unwrap();
    }
    let edition_field = if inherited {
        "edition.workspace = true".to_string()
    } else {
        format!("edition = \"{edition}\"")
    };
    fs_err::write(
        workspace.join("Cargo.toml"),
        format!(
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\n{edition_field}\n[workspace]\nmembers = [\"py\", \"shared\"]\n[workspace.package]\nedition = \"{edition}\"\n"
        ),
    )
    .unwrap();
    fs_err::write(workspace.join("src/lib.rs"), "").unwrap();
    fs_err::write(
        workspace.join("shared/Cargo.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[features]\nhost-only = []\n",
    )
    .unwrap();
    fs_err::write(
        workspace.join("shared/src/lib.rs"),
        "#[cfg(not(feature = \"host-only\"))]\npub fn target_only() {}\n",
    )
    .unwrap();
    fs_err::write(
        workspace.join("py/Cargo.toml"),
        "[package]\nname = \"py\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nshared = { path = \"../shared\" }\n[build-dependencies]\nshared = { path = \"../shared\", features = [\"host-only\"] }\n",
    )
    .unwrap();
    fs_err::write(
        workspace.join("py/src/main.rs"),
        "fn main() { shared::target_only(); }\n",
    )
    .unwrap();
    fs_err::write(workspace.join("py/build.rs"), "fn main() {}\n").unwrap();
    fs_err::write(
        workspace.join("py/pyproject.toml"),
        "[build-system]\nrequires = [\"maturin>=1,<2\"]\nbuild-backend = \"maturin\"\n[tool.maturin]\nbindings = \"bin\"\nlocked = true\n",
    )
    .unwrap();
    let run = |cwd: &Path, command: &mut Command| {
        let output = command
            .current_dir(cwd)
            .env("CARGO_HOME", temp_dir.path().join("cargo-home"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&workspace, Command::new("cargo").arg("generate-lockfile"));
    // Resolver 1 would unify host-only into the target dependency and remove
    // target_only(), making this build fail despite an otherwise valid lockfile.
    run(
        &workspace.join("py"),
        Command::new("cargo").args(["build", "--locked", "--offline"]),
    );
    let original_lockfile = fs_err::read(workspace.join("Cargo.lock")).unwrap();
    let dist = temp_dir.path().join("dist");
    run(
        &workspace.join("py"),
        Command::new(env!("CARGO_BIN_EXE_maturin"))
            .args(["sdist", "--out"])
            .arg(&dist),
    );
    assert_eq!(
        fs_err::read(workspace.join("Cargo.lock")).unwrap(),
        original_lockfile
    );
    let archive = fs_err::File::open(dist.join("py-0.1.0.tar.gz")).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let extracted = temp_dir.path().join("extracted");
    archive.unpack(&extracted).unwrap();
    let packaged = extracted.join("py-0.1.0");
    let manifest: toml::Value =
        toml::from_str(&fs_err::read_to_string(packaged.join("Cargo.toml")).unwrap()).unwrap();
    assert!(manifest.get("package").is_none());
    assert_eq!(manifest["workspace"]["resolver"].as_str(), Some(resolver));
    run(
        &packaged.join("py"),
        Command::new("cargo").args(["build", "--locked", "--offline"]),
    );
}

#[rstest::rstest]
#[case::no_default_features(false)]
#[case::target_filter(true)]
fn sdist_workspace_lockfile_uncached_dependency(#[case] target_filter: bool) {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path().join("workspace");
    for name in ["py", "unused"] {
        let path = workspace.join(name);
        fs_err::create_dir_all(path.join("src")).unwrap();
        let mut manifest =
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        if name == "py" {
            if target_filter {
                manifest.push_str(
                    "[target.'cfg(target_os = \"none\")'.dependencies]\nuncached = \"=1.0.0\"\n",
                );
            } else {
                manifest.push_str(
                    "[dependencies]\nuncached = { version = \"=1.0.0\", optional = true }\n[features]\ndefault = [\"dep:uncached\"]\n",
                );
            }
        }
        fs_err::write(path.join("Cargo.toml"), manifest).unwrap();
        fs_err::write(path.join("src/main.rs"), "fn main() {}\n").unwrap();
    }
    fs_err::write(
        workspace.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"py\", \"unused\"]\n",
    )
    .unwrap();
    let feature_options = if target_filter {
        ""
    } else {
        "no-default-features = true\n"
    };
    fs_err::write(
        workspace.join("py/pyproject.toml"),
        format!(
            "[build-system]\nrequires = [\"maturin>=1.0,<2.0\"]\nbuild-backend = \"maturin\"\n[tool.maturin]\nbindings = \"bin\"\nlocked = true\n{feature_options}"
        ),
    )
    .unwrap();

    // Supply only a local registry index. The disabled dependency can be
    // resolved into Cargo.lock, but its crate archive is deliberately absent.
    // Enabling default features or losing the target filter during staged
    // metadata would require that download and fail, without using the network.
    let index = temp_dir.path().join("registry/index/un/ca");
    fs_err::create_dir_all(&index).unwrap();
    fs_err::write(
        index.join("uncached"),
        format!(
            "{}\n",
            serde_json::json!({
                "name": "uncached",
                "vers": "1.0.0",
                "deps": [],
                "cksum": "0".repeat(64),
                "features": {},
                "yanked": false,
            })
        ),
    )
    .unwrap();
    fs_err::create_dir_all(workspace.join(".cargo")).unwrap();
    fs_err::write(
        workspace.join(".cargo/config.toml"),
        "[source.crates-io]\nreplace-with = \"test-registry\"\n[source.test-registry]\nlocal-registry = \"../registry\"\n",
    )
    .unwrap();
    let run = |command: &mut Command| {
        let output = command
            .current_dir(workspace.join("py"))
            .env("CARGO_HOME", temp_dir.path().join("cargo-home"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(Command::new("cargo").arg("generate-lockfile"));
    let original_contents = fs_err::read(workspace.join("Cargo.lock")).unwrap();
    let mut maturin = Command::new(env!("CARGO_BIN_EXE_maturin"));
    if target_filter {
        // The build command exposes target selection and verifies that the
        // generated archive also builds without the unavailable dependency.
        maturin
            .args(["build", "--sdist", "--target"])
            .arg(rustc_version::version_meta().unwrap().host)
            .args(["--compatibility", "linux"]);
    } else {
        maturin.arg("sdist");
    }
    let dist = temp_dir.path().join("dist");
    run(maturin.arg("--out").arg(&dist));
    assert_eq!(
        fs_err::read(workspace.join("Cargo.lock")).unwrap(),
        original_contents
    );
    let archive = fs_err::File::open(dist.join("py-0.1.0.tar.gz")).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let extracted = temp_dir.path().join("extracted");
    archive.unpack(&extracted).unwrap();
    let lockfile: toml::Value =
        toml::from_str(&fs_err::read_to_string(extracted.join("py-0.1.0/Cargo.lock")).unwrap())
            .unwrap();
    let names: Vec<_> = lockfile["package"]
        .as_array()
        .unwrap()
        .iter()
        .map(|package| package["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["py", "uncached"]);
}

#[rstest::rstest]
#[case::workspace_config_v3(false, 3)]
#[case::relative_config_v3(true, 3)]
#[case::workspace_config_v4(false, 4)]
#[case::relative_config_v4(true, 4)]
fn sdist_workspace_lockfile_pins(
    #[case] relative_config: bool,
    #[case] version: u32,
    #[values(
        "feature/sdist",
        "feature+sdist",
        "feature%2Fsdist",
        "feature&sdist",
        "master"
    )]
    branch: &str,
) {
    if which("git").is_err() {
        eprintln!("Skipping sdist_workspace_lockfile_pins: git not found");
        return;
    }

    fn run(command: &mut Command) -> std::process::Output {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{command:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn write_crate(path: &Path, name: &str, version: &str, dependencies: &str) {
        fs_err::create_dir_all(path.join("src")).unwrap();
        fs_err::write(
            path.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}\n"
            ),
        )
        .unwrap();
        let target = if name == "py" { "main.rs" } else { "lib.rs" };
        fs_err::write(path.join("src").join(target), "pub fn main() {}\n").unwrap();
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path().join("workspace");
    let cargo_home = temp_dir.path().join("cargo-home");
    let registry = temp_dir.path().join("registry");
    let git_dep = temp_dir.path().join("gitdep");
    write_crate(&git_dep, "gitdep", "0.1.0", "");
    let git = || {
        let mut command = Command::new("git");
        command.current_dir(&git_dep).args([
            "-c",
            "user.name=maturin-tests",
            "-c",
            "user.email=maturin-tests@example.com",
            "-c",
            "commit.gpgsign=false",
        ]);
        command
    };
    run(git().args(["init", &format!("--initial-branch={branch}"), "-q"]));
    run(git().args(["add", "."]));
    run(git().args(["commit", "-qm", "Initial dependency"]));
    // A second source for the same name/version exercises qualified source
    // references in dependency lists as well as package entries.
    run(git().args(["branch", "other"]));
    let git_url = Url::from_directory_path(&git_dep).unwrap();

    // A local registry exercises real archive checksums without network access.
    // Cargo before 1.87 cannot re-vendor directory sources (cargo#15260).
    let index = registry.join("index/re/gi");
    fs_err::create_dir_all(&index).unwrap();
    let add_registry_version = |version| {
        let name = format!("registry-dep-{version}");
        let path = temp_dir.path().join("registry-src").join(&name);
        write_crate(&path, "registry-dep", version, "");
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        archive.append_dir_all(&name, &path).unwrap();
        let archive = archive.into_inner().unwrap().finish().unwrap();
        let checksum = cargo_lock::Checksum::Sha256(Sha256::digest(&archive).into()).to_string();
        fs_err::write(registry.join(format!("{name}.crate")), archive).unwrap();
        let mut index = fs_err::OpenOptions::new()
            .create(true)
            .append(true)
            .open(index.join("registry-dep"))
            .unwrap();
        writeln!(
            index,
            "{}",
            serde_json::json!({
                "name": "registry-dep", "vers": version, "deps": [],
                "cksum": checksum, "features": {}, "yanked": false,
            })
        )
        .unwrap();
    };
    add_registry_version("1.0.0");
    add_registry_version("2.0.0");
    write_crate(
        &workspace.join("py"),
        "py",
        "0.1.0",
        &format!("registry-dep = \"1\"\ngitdep = {{ git = \"{git_url}\", branch = \"{branch}\" }}"),
    );
    write_crate(
        &workspace.join("unused"),
        "unused",
        "0.1.0",
        &format!("registry-dep = \"2\"\ngitdep = {{ git = \"{git_url}\", branch = \"other\" }}"),
    );
    fs_err::write(
        workspace.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"py\", \"unused\"]\n",
    )
    .unwrap();

    let config_option = if relative_config {
        // No automatically discovered config: every Cargo query must forward
        // this override to find the registry and preserve its relative paths.
        fs_err::create_dir_all(workspace.join("py/config")).unwrap();
        fs_err::write(
            workspace.join("py/config/overrides.toml"),
            "[source.crates-io]\nreplace-with = \"test-registry\"\n\n[source.test-registry]\nlocal-registry = \"../../registry\"\n",
        )
        .unwrap();
        "config = [\"config/overrides.toml\"]\n"
    } else {
        fs_err::create_dir_all(workspace.join(".cargo")).unwrap();
        fs_err::write(
            workspace.join(".cargo/config.toml"),
            "[source.crates-io]\nreplace-with = \"test-registry\"\n\n[source.test-registry]\nlocal-registry = \"../registry\"\n",
        )
        .unwrap();
        ""
    };
    let lock_option = if version == 3 { "frozen" } else { "locked" };
    fs_err::write(
        workspace.join("py/pyproject.toml"),
        format!(
            "[build-system]\nrequires = [\"maturin>=1.0,<2.0\"]\nbuild-backend = \"maturin\"\n\n[tool.maturin]\n{lock_option} = true\n{config_option}"
        ),
    )
    .unwrap();
    let cargo = || {
        let mut command = Command::new("cargo");
        command
            .current_dir(workspace.join("py"))
            .env("CARGO_HOME", &cargo_home);
        if relative_config {
            command.args(["--config", "config/overrides.toml"]);
        }
        command
    };
    run(cargo().arg("generate-lockfile"));
    let lock_path = workspace.join("Cargo.lock");
    let mut original = cargo_lock::Lockfile::load(&lock_path).unwrap();
    original.version = match version {
        3 => cargo_lock::ResolveVersion::V3,
        4 => cargo_lock::ResolveVersion::V4,
        _ => unreachable!(),
    };
    let original_contents = original.to_string();

    // Make newer compatible registry and Git versions available in the cache.
    // Re-resolving from scratch would silently lose the original pins.
    add_registry_version("1.1.0");
    fs_err::write(git_dep.join("src/lib.rs"), "pub fn newer() {}\n").unwrap();
    run(git().args(["commit", "-am", "Newer dependency", "-q"]));
    run(cargo().arg("update"));
    let newer = cargo_lock::Lockfile::load(&lock_path).unwrap();
    for name in ["registry-dep", "gitdep"] {
        assert!(newer.packages.iter().any(|package| {
            package.name.as_str() == name && !original.packages.contains(package)
        }));
    }
    fs_err::write(&lock_path, &original_contents).unwrap();
    run(cargo().args(["metadata", "--locked", "--offline", "--format-version", "1"]));

    let dist = temp_dir.path().join("dist");
    run(Command::new(env!("CARGO_BIN_EXE_maturin"))
        .args(["sdist", "--out"])
        .arg(&dist)
        .current_dir(workspace.join("py"))
        .env("CARGO_HOME", &cargo_home)
        .env("CARGO_NET_OFFLINE", "true"));
    assert_eq!(
        fs_err::read_to_string(&lock_path).unwrap(),
        original_contents,
        "building an sdist must not change the repository lockfile"
    );
    let archive = fs_err::File::open(dist.join("py-0.1.0.tar.gz")).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let extracted = temp_dir.path().join("extracted");
    archive.unpack(&extracted).unwrap();
    let packaged = extracted.join("py-0.1.0");
    // Compare pins in the original format without decoding legacy Git URLs.
    // cargo-lock's parser applies URL decoding even to v3's literal references.
    let packaged_lock: toml::Value =
        toml::from_str(&fs_err::read_to_string(packaged.join("Cargo.lock")).unwrap()).unwrap();
    let mut original_lock: toml::Value = toml::from_str(&original_contents).unwrap();
    assert_eq!(packaged_lock["version"], original_lock["version"]);
    original_lock["package"]
        .as_array_mut()
        .unwrap()
        .retain(|package| {
            package["name"].as_str() != Some("unused")
                && !(package["name"].as_str() == Some("registry-dep")
                    && package["version"].as_str() == Some("2.0.0"))
                && !package
                    .get("source")
                    .and_then(toml::Value::as_str)
                    .is_some_and(|source| source.contains("?branch=other#"))
        });
    let pins = |lockfile: &toml::Value| {
        lockfile["package"]
            .as_array()
            .unwrap()
            .iter()
            .map(|package| {
                ["name", "version", "source", "checksum"].map(|key| package.get(key).cloned())
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(pins(&packaged_lock), pins(&original_lock));
    assert!(!packaged.join("unused").exists());

    run(cargo()
        .args([
            "metadata",
            "--locked",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(packaged.join("Cargo.toml")));
    let vendor = run(cargo()
        .args([
            "vendor",
            "--locked",
            "--offline",
            "--respect-source-config",
            "--manifest-path",
        ])
        .arg(packaged.join("Cargo.toml"))
        .arg(packaged.join("VENDOR")));
    // Cargo itself cannot use vendored v3 Git sources with these characters,
    // even in the unmodified workspace (rust-lang/cargo#14584). The original
    // format, pins, locked metadata, and locked vendoring are checked above.
    if version == 3 && branch.contains(['+', '%', '&']) {
        return;
    }
    fs_err::create_dir_all(packaged.join(".cargo")).unwrap();
    fs_err::write(packaged.join(".cargo/config.toml"), vendor.stdout).unwrap();
    run(Command::new("cargo")
        .args(["metadata", "--frozen", "--format-version", "1"])
        .current_dir(&packaged)
        .env("CARGO_HOME", temp_dir.path().join("empty-cargo-home")));
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
