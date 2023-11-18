import multiprocessing
import os
import re
import shutil
from pathlib import Path
from typing import Generator

import pytest

from .common import (
    IMPORT_HOOK_HEADER,
    all_test_crate_names,
    create_project_from_blank_template,
    get_project_copy,
    install_editable,
    install_non_editable,
    is_installed_correctly,
    log,
    mixed_test_crate_names,
    remove_ansii_escape_characters,
    run_python,
    run_python_code,
    script_dir,
    test_crates,
    uninstall,
    with_underscores,
    handle_worker_process_error,
)

"""
These tests ensure the correct functioning of the project importer import hook.
The tests are intended to be run as part of the tests in `run.rs`
which provides a clean virtual environment for these tests to use.
"""

MATURIN_TEST_NAME = os.environ["MATURIN_TEST_NAME"]
MATURIN_BUILD_CACHE = test_crates / f"targets/import_hook_project_importer_build_cache_{MATURIN_TEST_NAME}"
# the CI does not have enough space to keep the outputs.
# When running locally you may set this to False for debugging
CLEAR_WORKSPACE = True

os.environ["CARGO_TARGET_DIR"] = str(test_crates / f"targets/import_hook_project_importer_{MATURIN_TEST_NAME}")
os.environ["MATURIN_BUILD_DIR"] = str(MATURIN_BUILD_CACHE)


@pytest.fixture()
def workspace(tmp_path: Path) -> Generator[Path, None, None]:
    try:
        yield tmp_path
    finally:
        if CLEAR_WORKSPACE:
            log(f"clearing workspace {tmp_path}")
            shutil.rmtree(tmp_path, ignore_errors=True)


def _clear_build_cache() -> None:
    if MATURIN_BUILD_CACHE.exists():
        log("clearing build cache")
        shutil.rmtree(MATURIN_BUILD_CACHE)


@pytest.mark.parametrize(
    "project_name",
    # path dependencies tested separately
    sorted(set(all_test_crate_names()) - {"pyo3-mixed-with-path-dep"}),
)
def test_install_from_script_inside(workspace: Path, project_name: str) -> None:
    """This test ensures that when a script is run from within a maturin project, the
    import hook can identify and install the containing project even if it is not
    already installed.

    limitation: if the project has python dependencies then those dependencies will be installed
    when the import hook triggers installation of the project but unlike the maturin project
    which the import hook handles specially, other installed projects may not become available
    until the interpreter is restarted (or the site module is reloaded)
    """
    _clear_build_cache()
    uninstall(project_name)

    project_dir = get_project_copy(test_crates / project_name, workspace / project_name)

    check_installed_dir = project_dir / "check_installed"
    check_installed_path = check_installed_dir / "check_installed.py"
    check_installed_path.write_text(f"{IMPORT_HOOK_HEADER}\n\n{check_installed_path.read_text()}")

    empty_dir = workspace / "empty"
    empty_dir.mkdir()

    output1, duration1 = run_python([str(check_installed_path)], cwd=empty_dir)
    assert "SUCCESS" in output1
    assert _rebuilt_message(project_name) in output1
    assert _up_to_date_message(project_name) not in output1

    assert is_installed_correctly(project_name, project_dir, "mixed" in project_name)

    output2, duration2 = run_python([str(check_installed_path)], cwd=empty_dir)
    assert "SUCCESS" in output2
    assert _rebuilt_message(project_name) not in output2
    assert _up_to_date_message(project_name) in output2

    assert duration2 < duration1

    assert is_installed_correctly(project_name, project_dir, "mixed" in project_name)


@pytest.mark.parametrize("project_name", ["pyo3-mixed", "pyo3-pure"])
def test_do_not_install_from_script_inside(workspace: Path, project_name: str) -> None:
    """This test ensures that when the import hook works correctly when it is
    configured to not rebuild/install projects if they aren't already installed.
    """
    _clear_build_cache()
    uninstall(project_name)

    project_dir = get_project_copy(test_crates / project_name, workspace / project_name)

    check_installed_path = project_dir / "check_installed/check_installed.py"
    header = """
import logging
logging.basicConfig(format='%(name)s [%(levelname)s] %(message)s', level=logging.DEBUG)

from maturin import import_hook
import_hook.reset_logger()
from maturin.import_hook import project_importer
project_importer.install(install_new_packages=False)
"""
    check_installed_path.write_text(f"{header}\n\n{check_installed_path.read_text()}")

    empty_dir = workspace / "empty"
    empty_dir.mkdir()

    output1, _ = run_python([str(check_installed_path)], cwd=empty_dir, expect_error=True, quiet=True)
    assert (
        f'package "{with_underscores(project_name)}" is not already '
        f"installed and install_new_packages=False. Not importing"
    ) in output1
    assert "SUCCESS" not in output1

    install_editable(project_dir)
    assert is_installed_correctly(project_name, project_dir, "mixed" in project_name)

    output2, _ = run_python([str(check_installed_path)], cwd=empty_dir)
    assert "SUCCESS" in output2
    assert f'package "{with_underscores(project_name)}" will be rebuilt because: no build status found' in output2
    assert _rebuilt_message(project_name) in output2

    output3, _ = run_python([str(check_installed_path)], cwd=empty_dir)
    assert "SUCCESS" in output3
    assert _rebuilt_message(project_name) not in output3
    assert _up_to_date_message(project_name) in output3


@pytest.mark.parametrize("project_name", ["pyo3-mixed", "pyo3-pure"])
def test_do_not_rebuild_if_installed_non_editable(workspace: Path, project_name: str) -> None:
    """This test ensures that if a maturin project is installed in non-editable
    mode then the import hook will not rebuild it or re-install it in editable mode.
    """
    _clear_build_cache()
    uninstall(project_name)
    project_dir = get_project_copy(test_crates / project_name, workspace / project_name)
    install_non_editable(project_dir)

    check_installed_outside_project = workspace / "check_installed"
    check_installed_outside_project.mkdir()

    check_installed_dir = project_dir / "check_installed"
    check_installed_path = check_installed_dir / "check_installed.py"
    header = """
import sys
import logging
logging.basicConfig(format='%(name)s [%(levelname)s] %(message)s', level=logging.DEBUG)
from maturin import import_hook
import_hook.reset_logger()
install_new_packages = len(sys.argv) > 1 and sys.argv[1] == 'INSTALL_NEW'
print(f'{install_new_packages=}')
import_hook.install(install_new_packages=install_new_packages)
"""
    check_installed_path.write_text(f"{header}\n\n{check_installed_path.read_text()}")
    shutil.copy(check_installed_path, check_installed_outside_project)

    (project_dir / "src/lib.rs").write_text("")  # will break once rebuilt

    # when outside the project, can still detect non-editable installed projects via dist-info
    output1, _ = run_python(["check_installed.py"], cwd=check_installed_outside_project)
    assert "SUCCESS" in output1
    assert "install_new_packages=False" in output1
    assert f'found project linked by dist-info: "{project_dir}"' in output1
    assert "package not installed in editable-mode and install_new_packages=False. not rebuilding" in output1

    # when inside the project, will detect the project above
    output2, _ = run_python(["check_installed.py"], cwd=check_installed_dir)
    assert "SUCCESS" in output2
    assert "install_new_packages=False" in output2
    assert "found project above the search path:" in output2
    assert "package not installed in editable-mode and install_new_packages=False. not rebuilding" in output2

    output3, _ = run_python(
        ["check_installed.py", "INSTALL_NEW"],
        cwd=check_installed_outside_project,
        quiet=True,
        expect_error=True,
    )
    assert "SUCCESS" not in output3
    assert "install_new_packages=True" in output3
    assert (
        f"ImportError: dynamic module does not define module "
        f"export function (PyInit_{with_underscores(project_name)})"
    ) in output3


@pytest.mark.parametrize("initially_mixed", [False, True])
@pytest.mark.parametrize(
    "project_name",
    # path dependencies tested separately
    sorted(set(all_test_crate_names()) - {"pyo3-mixed-with-path-dep"}),
)
def test_import_editable_installed_rebuild(workspace: Path, project_name: str, initially_mixed: bool) -> None:
    """This test ensures that an editable installed project is rebuilt when necessary if the import
    hook is active. This applies to mixed projects (which are installed as .pth files into
    site-packages when installed in editable mode) as well as pure projects (which are copied to site-packages
    when with a link back to the source directory when installed in editable mode).

    This is tested with the project initially being mixed and initially being pure to test that the import hook
    works even if the project changes significantly (eg from mixed to pure)
    """
    _clear_build_cache()
    uninstall(project_name)

    check_installed = (test_crates / project_name / "check_installed/check_installed.py").read_text()

    project_dir = create_project_from_blank_template(project_name, workspace / project_name, mixed=initially_mixed)

    log(f"installing blank project as {project_name}")

    install_editable(project_dir)
    assert is_installed_correctly(project_name, project_dir, initially_mixed)

    # without the import hook the installation test is expected to fail because the project should not be installed yet
    output0, _ = run_python_code(check_installed, quiet=True, expect_error=True)
    assert "AttributeError" in output0 or "ImportError" in output0 or "ModuleNotFoundError" in output0

    check_installed = f"{IMPORT_HOOK_HEADER}\n\n{check_installed}"

    log("overwriting blank project with genuine project without re-installing")
    shutil.rmtree(project_dir)
    get_project_copy(test_crates / project_name, project_dir)

    output1, duration1 = run_python_code(check_installed)
    assert "SUCCESS" in output1
    assert _rebuilt_message(project_name) in output1
    assert _up_to_date_message(project_name) not in output1

    assert is_installed_correctly(project_name, project_dir, "mixed" in project_name)

    output2, duration2 = run_python_code(check_installed)
    assert "SUCCESS" in output2
    assert _rebuilt_message(project_name) not in output2
    assert _up_to_date_message(project_name) in output2

    assert duration2 < duration1

    assert is_installed_correctly(project_name, project_dir, "mixed" in project_name)


@pytest.mark.parametrize(
    "project_name",
    # path dependencies tested separately
    sorted(set(mixed_test_crate_names()) - {"pyo3-mixed-with-path-dep"}),
)
def test_import_editable_installed_mixed_missing(workspace: Path, project_name: str) -> None:
    """This test ensures that editable installed mixed projects are rebuilt if they are imported
    and their artifacts are missing.

    This can happen when cleaning untracked files from git for example.

    This only affects mixed projects because artifacts of editable installed pure projects are
    copied to site-packages instead.
    """
    _clear_build_cache()
    uninstall(project_name)

    # making a copy because editable installation may write files into the project directory
    project_dir = get_project_copy(test_crates / project_name, workspace / project_name)
    project_backup_dir = get_project_copy(test_crates / project_name, workspace / f"backup_{project_name}")

    install_editable(project_dir)
    assert is_installed_correctly(project_name, project_dir, "mixed" in project_name)

    check_installed = test_crates / project_name / "check_installed/check_installed.py"

    log("checking that check_installed works without the import hook right after installing")
    output0, _ = run_python_code(check_installed.read_text())
    assert "SUCCESS" in output0

    check_installed_script = f"{IMPORT_HOOK_HEADER}\n\n{check_installed.read_text()}"

    shutil.rmtree(project_dir)
    shutil.copytree(project_backup_dir, project_dir)

    log("checking that the import hook rebuilds the project")

    output1, duration1 = run_python_code(check_installed_script)
    assert "SUCCESS" in output1
    assert _rebuilt_message(project_name) in output1
    assert _up_to_date_message(project_name) not in output1

    output2, duration2 = run_python_code(check_installed_script)
    assert "SUCCESS" in output2
    assert _rebuilt_message(project_name) not in output2
    assert _up_to_date_message(project_name) in output2

    assert duration2 < duration1

    assert is_installed_correctly(project_name, project_dir, True)


@pytest.mark.parametrize("mixed", [False, True])
@pytest.mark.parametrize("initially_mixed", [False, True])
def test_concurrent_import(workspace: Path, initially_mixed: bool, mixed: bool) -> None:
    """This test ensures that if multiple scripts attempt to use the import hook concurrently,
    that the project still installs correctly and does not crash.

    This test uses a blank project initially to ensure that a rebuild is necessary to be
    able to use the project.
    """
    if mixed:
        project_name = "pyo3-mixed"
        check_installed = """
import pyo3_mixed
assert pyo3_mixed.get_42() == 42
print('SUCCESS')
"""
    else:
        project_name = "pyo3-pure"
        check_installed = """
import pyo3_pure
assert pyo3_pure.DummyClass.get_42() == 42
print('SUCCESS')
"""

    _clear_build_cache()
    uninstall(project_name)

    check_installed_with_hook = f"{IMPORT_HOOK_HEADER}\n\n{check_installed}"

    project_dir = create_project_from_blank_template(project_name, workspace / project_name, mixed=initially_mixed)

    log(f"initially mixed: {initially_mixed}, mixed: {mixed}")
    log(f"installing blank project as {project_name}")

    install_editable(project_dir)
    assert is_installed_correctly(project_name, project_dir, initially_mixed)

    shutil.rmtree(project_dir)
    get_project_copy(test_crates / project_name, project_dir)

    args = {"python_script": check_installed_with_hook, "quiet": True}
    with multiprocessing.Pool(processes=3) as pool:
        p1 = pool.apply_async(run_python_code, kwds=args)
        p2 = pool.apply_async(run_python_code, kwds=args)
        p3 = pool.apply_async(run_python_code, kwds=args)

        with handle_worker_process_error():
            output_1, duration_1 = p1.get()

        with handle_worker_process_error():
            output_2, duration_2 = p2.get()

        with handle_worker_process_error():
            output_3, duration_3 = p3.get()

    log("output 1")
    log(output_1)
    log("output 2")
    log(output_2)
    log("output 3")
    log(output_3)

    num_compilations = 0
    num_up_to_date = 0
    num_waiting = 0
    for output in [output_1, output_2, output_3]:
        assert "SUCCESS" in output

        if "waiting on lock" in output:
            num_waiting += 1

        if _up_to_date_message(project_name) in output:
            num_up_to_date += 1

        if _rebuilt_message(project_name) in output:
            num_compilations += 1

    assert num_compilations == 1
    assert num_up_to_date == 2
    assert num_waiting == 2

    assert is_installed_correctly(project_name, project_dir, mixed)


def test_import_multiple_projects(workspace: Path) -> None:
    """This test ensures that the import hook can be used to load multiple projects
    in the same run.

    A single pair of projects is chosen for this test because it should not make
    any difference which projects are imported
    """
    _clear_build_cache()
    uninstall("pyo3-mixed")
    uninstall("pyo3-pure")

    mixed_dir = create_project_from_blank_template("pyo3-mixed", workspace / "pyo3-mixed", mixed=True)
    pure_dir = create_project_from_blank_template("pyo3-pure", workspace / "pyo3-pure", mixed=False)

    install_editable(mixed_dir)
    assert is_installed_correctly("pyo3-mixed", mixed_dir, True)
    install_editable(pure_dir)
    assert is_installed_correctly("pyo3-pure", pure_dir, False)

    shutil.rmtree(mixed_dir)
    shutil.rmtree(pure_dir)
    get_project_copy(test_crates / "pyo3-mixed", mixed_dir)
    get_project_copy(test_crates / "pyo3-pure", pure_dir)

    check_installed = "{}\n\n{}\n\n{}".format(
        IMPORT_HOOK_HEADER,
        (mixed_dir / "check_installed/check_installed.py").read_text(),
        (pure_dir / "check_installed/check_installed.py").read_text(),
    )

    output1, duration1 = run_python_code(check_installed)
    assert "SUCCESS" in output1
    assert _rebuilt_message("pyo3-mixed") in output1
    assert _rebuilt_message("pyo3-pure") in output1
    assert _up_to_date_message("pyo3-mixed") not in output1
    assert _up_to_date_message("pyo3-pure") not in output1

    output2, duration2 = run_python_code(check_installed)
    assert "SUCCESS" in output2
    assert _rebuilt_message("pyo3-mixed") not in output2
    assert _rebuilt_message("pyo3-pure") not in output2
    assert _up_to_date_message("pyo3-mixed") in output2
    assert _up_to_date_message("pyo3-pure") in output2

    assert duration2 < duration1

    assert is_installed_correctly("pyo3-mixed", mixed_dir, True)
    assert is_installed_correctly("pyo3-pure", pure_dir, False)


def test_rebuild_on_change_to_path_dependency(workspace: Path) -> None:
    """This test ensures that the imported project is rebuilt if any of its path
    dependencies are edited.
    """
    _clear_build_cache()
    project_name = "pyo3-mixed-with-path-dep"
    uninstall(project_name)

    project_dir = get_project_copy(test_crates / project_name, workspace / project_name)
    get_project_copy(test_crates / "some_path_dep", workspace / "some_path_dep")
    transitive_dep_dir = get_project_copy(test_crates / "transitive_path_dep", workspace / "transitive_path_dep")

    install_editable(project_dir)
    assert is_installed_correctly(project_name, project_dir, True)

    check_installed = f"""
{IMPORT_HOOK_HEADER}

import pyo3_mixed_with_path_dep

assert pyo3_mixed_with_path_dep.get_42() == 42, 'get_42 did not return 42'

print('21 is half 42:', pyo3_mixed_with_path_dep.is_half(21, 42))
print('21 is half 63:', pyo3_mixed_with_path_dep.is_half(21, 63))
"""

    output1, duration1 = run_python_code(check_installed)
    assert "21 is half 42: True" in output1
    assert "21 is half 63: False" in output1

    transitive_dep_lib = transitive_dep_dir / "src/lib.rs"
    transitive_dep_lib.write_text(transitive_dep_lib.read_text().replace("x + y == sum", "x + x + y == sum"))

    output2, duration2 = run_python_code(check_installed)
    assert "21 is half 42: False" in output2
    assert "21 is half 63: True" in output2

    assert is_installed_correctly(project_name, project_dir, True)


@pytest.mark.parametrize("is_mixed", [False, True])
def test_rebuild_on_settings_change(workspace: Path, is_mixed: bool) -> None:
    """When the source code has not changed but the import hook uses different maturin flags
    the project is rebuilt.
    """
    _clear_build_cache()
    uninstall("my-script")

    project_dir = create_project_from_blank_template("my-script", workspace / "my-script", mixed=is_mixed)
    shutil.copy(script_dir / "rust_file_import/my_script_3.rs", project_dir / "src/lib.rs")
    manifest_path = project_dir / "Cargo.toml"
    manifest_path.write_text(f"{manifest_path.read_text()}\n[features]\nlarge_number = []\n")

    install_editable(project_dir)
    assert is_installed_correctly("my-script", project_dir, is_mixed)

    helper_path = script_dir / "rust_file_import/rebuild_on_settings_change_helper.py"

    output1, _ = run_python([str(helper_path)], cwd=workspace)
    assert "building with default settings" in output1
    assert "get_num = 10" in output1
    assert "SUCCESS" in output1
    assert 'package "my_script" will be rebuilt because: no build status found' in output1

    output2, _ = run_python([str(helper_path)], cwd=workspace)
    assert "get_num = 10" in output2
    assert "SUCCESS" in output2
    assert 'package up to date: "my_script"' in output2

    output3, _ = run_python([str(helper_path), "LARGE_NUMBER"], cwd=workspace)
    assert "building with large_number feature enabled" in output3
    assert (
        'package "my_script" will be rebuilt because: current maturin args do not match the previous build'
    ) in output3
    assert "get_num = 100" in output3
    assert "SUCCESS" in output3

    output4, _ = run_python([str(helper_path), "LARGE_NUMBER"], cwd=workspace)
    assert "building with large_number feature enabled" in output4
    assert 'package up to date: "my_script"' in output4
    assert "get_num = 100" in output4
    assert "SUCCESS" in output4


class TestLogging:
    """These tests ensure that the desired messages are visible to the user in the default logging configuration."""

    loader_script = """\
import sys
from maturin import import_hook

if len(sys.argv) > 1 and sys.argv[1] == 'RESET_LOGGER':
    import_hook.reset_logger()

import_hook.install()

try:
    import test_project
except ImportError as e:
    # catch instead of printing the traceback since that may depend on the interpreter
    print(f'caught ImportError: {e}')
else:
    print("value", test_project.value)
    print("SUCCESS")
"""

    @staticmethod
    def _create_clean_project(tmp_dir: Path, is_mixed: bool) -> Path:
        _clear_build_cache()
        uninstall("test-project")
        project_dir = create_project_from_blank_template("test-project", tmp_dir / "test-project", mixed=is_mixed)
        install_editable(project_dir)
        assert is_installed_correctly("test-project", project_dir, is_mixed)

        lib_path = project_dir / "src/lib.rs"
        lib_src = lib_path.read_text().replace("_m:", "m:").replace("Ok(())", 'm.add("value", 10)?;Ok(())')
        lib_path.write_text(lib_src)

        return project_dir

    @pytest.mark.parametrize("is_mixed", [False, True])
    def test_default_rebuild(self, workspace: Path, is_mixed: bool) -> None:
        """By default, when a module is out of date the import hook logs messages
        before and after rebuilding but hides the underlying details.
        """
        self._create_clean_project(workspace, is_mixed)

        output, _ = run_python_code(self.loader_script)
        pattern = (
            'building "test_project"\n'
            'rebuilt and loaded package "test_project" in [0-9.]+s\n'
            "value 10\n"
            "SUCCESS\n"
        )
        assert re.fullmatch(pattern, output, flags=re.MULTILINE) is not None

    @pytest.mark.parametrize("is_mixed", [False, True])
    def test_default_up_to_date(self, workspace: Path, is_mixed: bool) -> None:
        """By default, when the module is up-to-date nothing is printed."""
        self._create_clean_project(workspace / "project", is_mixed)

        run_python_code(self.loader_script)  # run once to rebuild

        output, _ = run_python_code(self.loader_script)
        assert output == "value 10\nSUCCESS\n"

    @pytest.mark.parametrize("is_mixed", [False, True])
    def test_default_compile_error(self, workspace: Path, is_mixed: bool) -> None:
        """If compilation fails then the error message from maturin is printed and an ImportError is raised."""
        project_dir = self._create_clean_project(workspace / "project", is_mixed)

        lib_path = project_dir / "src/lib.rs"
        lib_path.write_text(lib_path.read_text().replace("Ok(())", ""))

        output, _ = run_python_code(self.loader_script)
        pattern = (
            'building "test_project"\n'
            'maturin\\.import_hook \\[ERROR\\] command ".*" returned non-zero exit status: 1\n'
            "maturin\\.import_hook \\[ERROR\\] maturin output:\n"
            ".*"
            "expected `Result<\\(\\), PyErr>`, found `\\(\\)`"
            ".*"
            "maturin failed"
            ".*"
            "caught ImportError: Failed to build package with maturin\n"
        )
        assert re.fullmatch(pattern, output, flags=re.MULTILINE | re.DOTALL) is not None

    @pytest.mark.parametrize("is_mixed", [False, True])
    def test_default_compile_warning(self, workspace: Path, is_mixed: bool) -> None:
        """If compilation succeeds with warnings then the output of maturin is printed.
        If the module is already up to date but warnings were raised when it was first
        built, the warnings will be printed again.
        """
        project_dir = self._create_clean_project(workspace / "project", is_mixed)
        lib_path = project_dir / "src/lib.rs"
        lib_path.write_text(lib_path.read_text().replace("Ok(())", "#[warn(unused_variables)]{let x = 12;}; Ok(())"))

        output1, _ = run_python_code(self.loader_script)
        output1 = remove_ansii_escape_characters(output1)
        pattern = (
            'building "test_project"\n'
            'maturin.import_hook \\[WARNING\\] build of "test_project" succeeded with warnings:\n'
            ".*"
            "warning: unused variable: `x`"
            ".*"
            'rebuilt and loaded package "test_project" in [0-9.]+s\n'
            "value 10\n"
            "SUCCESS\n"
        )
        assert re.fullmatch(pattern, output1, flags=re.MULTILINE | re.DOTALL) is not None

        output2, _ = run_python_code(self.loader_script)
        output2 = remove_ansii_escape_characters(output2)
        pattern = (
            'maturin.import_hook \\[WARNING\\] the last build of "test_project" succeeded with warnings:\n'
            ".*"
            "warning: unused variable: `x`"
            ".*"
            "value 10\n"
            "SUCCESS\n"
        )
        assert re.fullmatch(pattern, output2, flags=re.MULTILINE | re.DOTALL) is not None

    @pytest.mark.parametrize("is_mixed", [False, True])
    def test_reset_logger_without_configuring(self, workspace: Path, is_mixed: bool) -> None:
        """If reset_logger is called then by default logging level INFO is not printed
        (because the messages are handled by the root logger).
        """
        self._create_clean_project(workspace / "project", is_mixed)
        output, _ = run_python_code(self.loader_script, args=["RESET_LOGGER"])
        assert output == "value 10\nSUCCESS\n"

    @pytest.mark.parametrize("is_mixed", [False, True])
    def test_successful_compilation_but_not_valid(self, workspace: Path, is_mixed: bool) -> None:
        """If the project compiles but does not import correctly an ImportError is raised."""
        project_dir = self._create_clean_project(workspace / "project", is_mixed)
        lib_path = project_dir / "src/lib.rs"
        lib_path.write_text(lib_path.read_text().replace("test_project", "test_project_new_name"))

        output, _ = run_python_code(self.loader_script, quiet=True)
        pattern = (
            'building "test_project"\n'
            'rebuilt and loaded package "test_project" in [0-9.]+s\n'
            "caught ImportError: dynamic module does not define module export function \\(PyInit_test_project\\)\n"
        )
        assert re.fullmatch(pattern, output, flags=re.MULTILINE) is not None


def _up_to_date_message(project_name: str) -> str:
    return f'package up to date: "{with_underscores(project_name)}"'


def _rebuilt_message(project_name: str) -> str:
    return f'rebuilt and loaded package "{with_underscores(project_name)}"'
