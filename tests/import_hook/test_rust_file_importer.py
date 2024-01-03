import os
import re
import shutil
from pathlib import Path
from typing import Generator, Tuple

import pytest

from .common import (
    log,
    remove_ansii_escape_characters,
    run_python,
    script_dir,
    test_crates,
    run_concurrent_python,
    check_match,
    missing_entrypoint_error_message_pattern,
)

"""
These tests ensure the correct functioning of the rust file importer import hook.
The tests are intended to be run as part of the tests in `run.rs`
which provides a clean virtual environment for these tests to use.
"""

MATURIN_TEST_NAME = os.environ["MATURIN_TEST_NAME"]
MATURIN_BUILD_CACHE = test_crates / f"targets/import_hook_file_importer_build_cache_{MATURIN_TEST_NAME}"
# the CI does not have enough space to keep the outputs.
# When running locally you may set this to False for debugging
CLEAR_WORKSPACE = True

os.environ["CARGO_TARGET_DIR"] = str(test_crates / f"targets/import_hook_file_importer_{MATURIN_TEST_NAME}")
os.environ["MATURIN_BUILD_DIR"] = str(MATURIN_BUILD_CACHE)


def _clear_build_cache() -> None:
    if MATURIN_BUILD_CACHE.exists():
        log("clearing build cache")
        shutil.rmtree(MATURIN_BUILD_CACHE)


@pytest.fixture()
def workspace(tmp_path: Path) -> Generator[Path, None, None]:
    try:
        yield tmp_path
    finally:
        if CLEAR_WORKSPACE:
            log(f"clearing workspace {tmp_path}")
            shutil.rmtree(tmp_path, ignore_errors=True)


def test_absolute_import(workspace: Path) -> None:
    """Test imports of the form `import ab.cd.ef`."""
    _clear_build_cache()

    helper_path = script_dir / "rust_file_import/absolute_import_helper.py"

    output1, duration1 = run_python([str(helper_path)], cwd=workspace)
    assert "SUCCESS" in output1
    assert "module up to date" not in output1
    assert "creating project for" in output1

    output2, duration2 = run_python([str(helper_path)], cwd=workspace)
    assert "SUCCESS" in output2
    assert "module up to date" in output2
    assert "creating project for" not in output2

    assert duration2 < duration1


def test_relative_import() -> None:
    """Test imports of the form `from .ab import cd`."""
    _clear_build_cache()

    output1, duration1 = run_python(["-m", "rust_file_import.relative_import_helper"], cwd=script_dir)
    assert "SUCCESS" in output1
    assert "module up to date" not in output1
    assert "creating project for" in output1

    output2, duration2 = run_python(["-m", "rust_file_import.relative_import_helper"], cwd=script_dir)
    assert "SUCCESS" in output2
    assert "module up to date" in output2
    assert "creating project for" not in output2

    assert duration2 < duration1


def test_top_level_import(workspace: Path) -> None:
    """Test imports of the form `import ab`."""
    _clear_build_cache()

    helper_path = script_dir / "rust_file_import/packages/top_level_import_helper.py"

    output1, duration1 = run_python([str(helper_path)], cwd=workspace)
    assert "SUCCESS" in output1
    assert "module up to date" not in output1
    assert "creating project for" in output1

    output2, duration2 = run_python([str(helper_path)], cwd=workspace)
    assert "SUCCESS" in output2
    assert "module up to date" in output2
    assert "creating project for" not in output2

    assert duration2 < duration1


def test_multiple_imports(workspace: Path) -> None:
    """Test importing the same rs file multiple times by different names in the same session."""
    _clear_build_cache()

    helper_path = script_dir / "rust_file_import/multiple_import_helper.py"

    output, _ = run_python([str(helper_path)], cwd=workspace)
    assert "SUCCESS" in output
    assert 'rebuilt and loaded module "packages.subpackage.my_rust_module"' in output
    assert output.count("importing rust file") == 1


def test_concurrent_import() -> None:
    """Test multiple processes attempting to import the same modules at the same time."""
    _clear_build_cache()
    args = {
        "args": ["rust_file_import/concurrent_import_helper.py"],
        "cwd": script_dir,
        "quiet": True,
    }

    outputs = run_concurrent_python(3, run_python, args)

    assert all(o.success for o in outputs)

    num_compilations = 0
    num_up_to_date = 0
    num_waiting = 0
    for output in outputs:
        assert "SUCCESS" in output.output
        assert "importing rust file" in output.output
        if "waiting on lock" in output.output:
            num_waiting += 1
        if "creating project for" in output.output:
            num_compilations += 1
        if "module up to date" in output.output:
            num_up_to_date += 1

    assert num_compilations == 1
    assert num_up_to_date == 2
    assert num_waiting == 2


def test_rebuild_on_change(workspace: Path) -> None:
    """Test that modules are rebuilt if they are edited."""
    _clear_build_cache()

    script_path = workspace / "my_script.rs"
    helper_path = shutil.copy(script_dir / "rust_file_import/rebuild_on_change_helper.py", workspace)

    shutil.copy(script_dir / "rust_file_import/my_script_1.rs", script_path)

    output1, _ = run_python([str(helper_path)], cwd=workspace)
    assert "get_num = 10" in output1
    assert "failed to import get_other_num" in output1
    assert "SUCCESS" in output1

    assert "module up to date" not in output1
    assert "creating project for" in output1

    shutil.copy(script_dir / "rust_file_import/my_script_2.rs", script_path)

    output2, _ = run_python([str(helper_path)], cwd=workspace)
    assert "get_num = 20" in output2
    assert "get_other_num = 100" in output2
    assert "SUCCESS" in output2

    assert "module up to date" not in output2
    assert "creating project for" in output2


def test_rebuild_on_settings_change(workspace: Path) -> None:
    """Test that modules are rebuilt if the settings (eg maturin flags) used by the import hook changes."""
    _clear_build_cache()

    script_path = workspace / "my_script.rs"
    helper_path = shutil.copy(script_dir / "rust_file_import/rebuild_on_settings_change_helper.py", workspace)

    shutil.copy(script_dir / "rust_file_import/my_script_3.rs", script_path)

    output1, _ = run_python([str(helper_path)], cwd=workspace)
    assert "get_num = 10" in output1
    assert "SUCCESS" in output1
    assert "building with default settings" in output1
    assert "module up to date" not in output1
    assert "creating project for" in output1

    output2, _ = run_python([str(helper_path)], cwd=workspace)
    assert "get_num = 10" in output2
    assert "SUCCESS" in output2
    assert "module up to date" in output2

    output3, _ = run_python([str(helper_path), "LARGE_NUMBER"], cwd=workspace)
    assert "building with large_number feature enabled" in output3
    assert "module up to date" not in output3
    assert "creating project for" in output3
    assert "get_num = 100" in output3
    assert "SUCCESS" in output3

    output4, _ = run_python([str(helper_path), "LARGE_NUMBER"], cwd=workspace)
    assert "building with large_number feature enabled" in output4
    assert "module up to date" in output4
    assert "get_num = 100" in output4
    assert "SUCCESS" in output4


class TestLogging:
    """test the desired messages are visible to the user in the default logging configuration."""

    loader_script = """\
import sys
from maturin import import_hook

if len(sys.argv) > 1 and sys.argv[1] == 'RESET_LOGGER':
    import_hook.reset_logger()

import_hook.install()

try:
    import my_script
except ImportError as e:
    # catch instead of printing the traceback since that may depend on the interpreter
    print(f'caught ImportError: {e}')
else:
    print("get_num", my_script.get_num())
    print("SUCCESS")
"""

    def _create_clean_package(self, package_path: Path) -> Tuple[Path, Path]:
        _clear_build_cache()

        package_path.mkdir()
        original_script_path = script_dir / "rust_file_import/my_script_1.rs"
        rs_path = Path(shutil.copy(original_script_path, package_path / "my_script.rs"))
        py_path = package_path / "loader.py"
        py_path.write_text(self.loader_script)
        return rs_path, py_path

    def test_default_rebuild(self, workspace: Path) -> None:
        """By default, when a module is out of date the import hook logs messages
        before and after rebuilding but hides the underlying details.
        """
        rs_path, py_path = self._create_clean_package(workspace / "package")

        output, _ = run_python([str(py_path)], workspace)
        pattern = 'building "my_script"\nrebuilt and loaded module "my_script" in [0-9.]+s\nget_num 10\nSUCCESS\n'
        check_match(output, pattern, flags=re.MULTILINE)

    def test_default_up_to_date(self, workspace: Path) -> None:
        """By default, when the module is up-to-date nothing is printed."""
        rs_path, py_path = self._create_clean_package(workspace / "package")

        run_python([str(py_path)], workspace)  # run once to rebuild

        output, _ = run_python([str(py_path)], workspace)
        assert output == "get_num 10\nSUCCESS\n"

    def test_default_compile_error(self, workspace: Path) -> None:
        """If compilation fails then the error message from maturin is printed and an ImportError is raised."""
        rs_path, py_path = self._create_clean_package(workspace / "package")

        rs_path.write_text(rs_path.read_text().replace("10", ""))
        output, _ = run_python([str(py_path)], workspace, quiet=True)
        pattern = (
            'building "my_script"\n'
            'maturin\\.import_hook \\[ERROR\\] command ".*" returned non-zero exit status: 1\n'
            "maturin\\.import_hook \\[ERROR\\] maturin output:\n"
            ".*"
            "expected `usize`, found `\\(\\)`"
            ".*"
            "maturin failed"
            ".*"
            "caught ImportError: Failed to build wheel with maturin\n"
        )
        check_match(output, pattern, flags=re.MULTILINE | re.DOTALL)

    def test_default_compile_warning(self, workspace: Path) -> None:
        """If compilation succeeds with warnings then the output of maturin is printed.
        If the module is already up to date but warnings were raised when it was first
        built, the warnings will be printed again.
        """
        rs_path, py_path = self._create_clean_package(workspace / "package")
        rs_path.write_text(rs_path.read_text().replace("10", "#[warn(unused_variables)]{let x = 12;}; 20"))

        output1, _ = run_python([str(py_path)], workspace)
        output1 = remove_ansii_escape_characters(output1)
        pattern = (
            'building "my_script"\n'
            'maturin.import_hook \\[WARNING\\] build of "my_script" succeeded with warnings:\n'
            ".*"
            "warning: unused variable: `x`"
            ".*"
            'rebuilt and loaded module "my_script" in [0-9.]+s\n'
            "get_num 20\n"
            "SUCCESS\n"
        )
        check_match(output1, pattern, flags=re.MULTILINE | re.DOTALL)

        output2, _ = run_python([str(py_path)], workspace)
        output2 = remove_ansii_escape_characters(output2)
        pattern = (
            'maturin.import_hook \\[WARNING\\] the last build of "my_script" succeeded with warnings:\n'
            ".*"
            "warning: unused variable: `x`"
            ".*"
            "get_num 20\n"
            "SUCCESS\n"
        )
        check_match(output2, pattern, flags=re.MULTILINE | re.DOTALL)

    def test_reset_logger_without_configuring(self, workspace: Path) -> None:
        """If reset_logger is called then by default logging level INFO is not printed
        (because the messages are handled by the root logger).
        """
        rs_path, py_path = self._create_clean_package(workspace / "package")
        output, _ = run_python([str(py_path), "RESET_LOGGER"], workspace)
        assert output == "get_num 10\nSUCCESS\n"

    def test_successful_compilation_but_not_valid(self, workspace: Path) -> None:
        """If the script compiles but does not import correctly an ImportError is raised."""
        rs_path, py_path = self._create_clean_package(workspace / "package")
        rs_path.write_text(rs_path.read_text().replace("my_script", "my_script_new_name"))
        output, _ = run_python([str(py_path)], workspace, quiet=True)
        pattern = (
            'building "my_script"\n'
            'rebuilt and loaded module "my_script" in [0-9.]+s\n'
            f"caught ImportError: {missing_entrypoint_error_message_pattern('my_script')}\n"
        )
        check_match(output, pattern, flags=re.MULTILINE)
