import multiprocessing
import os
import re
import shutil
from pathlib import Path
from typing import Tuple

from .common import log, run_python, script_dir, test_crates

"""
These tests ensure the correct functioning of the rust file importer import hook.
The tests are intended to be run as part of the tests in `run.rs`
which provides a clean virtual environment for these tests to use.
"""

MATURIN_BUILD_CACHE = test_crates / "targets/import_hook_file_importer_build_cache"

os.environ["CARGO_TARGET_DIR"] = str(test_crates / "targets/import_hook_file_importer")
os.environ["MATURIN_BUILD_DIR"] = str(MATURIN_BUILD_CACHE)


def _clear_build_cache() -> None:
    if MATURIN_BUILD_CACHE.exists():
        log("clearing build cache")
        shutil.rmtree(MATURIN_BUILD_CACHE)


def test_absolute_import(tmp_path: Path) -> None:
    """test imports of the form `import ab.cd.ef`"""
    _clear_build_cache()

    helper_path = script_dir / "rust_file_import/absolute_import_helper.py"

    output1, duration1 = run_python([str(helper_path)], cwd=tmp_path)
    assert "SUCCESS" in output1
    assert "module up to date" not in output1
    assert "creating project for" in output1

    output2, duration2 = run_python([str(helper_path)], cwd=tmp_path)
    assert "SUCCESS" in output2
    assert "module up to date" in output2
    assert "creating project for" not in output2

    assert duration2 < duration1


def test_relative_import(tmp_path: Path) -> None:
    """test imports of the form `from .ab import cd`"""
    _clear_build_cache()

    output1, duration1 = run_python(
        ["-m", "rust_file_import.relative_import_helper"], cwd=script_dir
    )
    assert "SUCCESS" in output1
    assert "module up to date" not in output1
    assert "creating project for" in output1

    output2, duration2 = run_python(
        ["-m", "rust_file_import.relative_import_helper"], cwd=script_dir
    )
    assert "SUCCESS" in output2
    assert "module up to date" in output2
    assert "creating project for" not in output2

    assert duration2 < duration1


def test_top_level_import(tmp_path: Path) -> None:
    """test imports of the form `import ab`"""
    _clear_build_cache()

    helper_path = script_dir / "rust_file_import/packages/top_level_import_helper.py"

    output1, duration1 = run_python([str(helper_path)], cwd=tmp_path)
    assert "SUCCESS" in output1
    assert "module up to date" not in output1
    assert "creating project for" in output1

    output2, duration2 = run_python([str(helper_path)], cwd=tmp_path)
    assert "SUCCESS" in output2
    assert "module up to date" in output2
    assert "creating project for" not in output2

    assert duration2 < duration1


def test_multiple_imports(tmp_path: Path) -> None:
    """test importing the same rs file multiple times by different names in the same session"""
    _clear_build_cache()

    helper_path = script_dir / "rust_file_import/multiple_import_helper.py"

    output, _ = run_python([str(helper_path)], cwd=tmp_path)
    assert "SUCCESS" in output
    assert 'rebuilt and loaded module "packages.subpackage.my_rust_module"' in output
    assert output.count("importing rust file") == 1


def test_concurrent_import() -> None:
    """test multiple processes attempting to import the same modules at the same time"""
    _clear_build_cache()
    args = {
        "args": ["rust_file_import/concurrent_import_helper.py"],
        "cwd": script_dir,
        "quiet": True,
    }

    with multiprocessing.Pool(processes=3) as pool:
        p1 = pool.apply_async(run_python, kwds=args)
        p2 = pool.apply_async(run_python, kwds=args)
        p3 = pool.apply_async(run_python, kwds=args)

        output_1, duration_1 = p1.get()
        output_2, duration_2 = p2.get()
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
        assert "importing rust file" in output
        if "waiting on lock" in output:
            num_waiting += 1
        if "creating project for" in output:
            num_compilations += 1
        if "module up to date" in output:
            num_up_to_date += 1

    assert num_compilations == 1
    assert num_up_to_date == 2
    assert num_waiting == 2


def test_rebuild_on_change(tmp_path: Path) -> None:
    """test that modules are rebuilt if they are edited"""
    _clear_build_cache()

    script_path = tmp_path / "my_script.rs"
    helper_path = shutil.copy(
        script_dir / "rust_file_import/rebuild_on_change_helper.py", tmp_path
    )

    shutil.copy(script_dir / "rust_file_import/my_script_1.rs", script_path)

    output1, _ = run_python([str(helper_path)], cwd=tmp_path)
    assert "get_num = 10" in output1
    assert "failed to import get_other_num" in output1
    assert "SUCCESS" in output1

    assert "module up to date" not in output1
    assert "creating project for" in output1

    shutil.copy(script_dir / "rust_file_import/my_script_2.rs", script_path)

    output2, _ = run_python([str(helper_path)], cwd=tmp_path)
    assert "get_num = 20" in output2
    assert "get_other_num = 100" in output2
    assert "SUCCESS" in output2

    assert "module up to date" not in output2
    assert "creating project for" in output2


def test_rebuild_on_settings_change(tmp_path: Path) -> None:
    """test that modules are rebuilt if the settings (eg maturin flags) used by the import hook changes"""
    _clear_build_cache()

    script_path = tmp_path / "my_script.rs"
    helper_path = shutil.copy(
        script_dir / "rust_file_import/rebuild_on_settings_change_helper.py", tmp_path
    )

    shutil.copy(script_dir / "rust_file_import/my_script_3.rs", script_path)

    output1, _ = run_python([str(helper_path)], cwd=tmp_path)
    assert "get_num = 10" in output1
    assert "SUCCESS" in output1
    assert "building my_script with default settings" in output1
    assert "module up to date" not in output1
    assert "creating project for" in output1

    output2, _ = run_python([str(helper_path)], cwd=tmp_path)
    assert "get_num = 10" in output2
    assert "SUCCESS" in output2
    assert "module up to date" in output2

    output3, _ = run_python([str(helper_path), "LARGE_NUMBER"], cwd=tmp_path)
    assert "building my_script with large_number feature enabled" in output3
    assert "module up to date" not in output3
    assert "creating project for" in output3
    assert "get_num = 100" in output3
    assert "SUCCESS" in output3

    output4, _ = run_python([str(helper_path), "LARGE_NUMBER"], cwd=tmp_path)
    assert "building my_script with large_number feature enabled" in output4
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

    def test_default_rebuild(self, tmp_path: Path) -> None:
        """By default, when a module is out of date the import hook logs messages
        before and after rebuilding but hides the underlying details.
        """
        rs_path, py_path = self._create_clean_package(tmp_path / "package")

        output, _ = run_python([str(py_path)], tmp_path)
        pattern = (
            'building "my_script"\n'
            'rebuilt and loaded module "my_script" in [0-9.]+s\n'
            "get_num 10\n"
            "SUCCESS\n"
        )
        assert re.fullmatch(pattern, output, flags=re.MULTILINE) is not None

    def test_default_up_to_date(self, tmp_path: Path) -> None:
        """By default, when the module is up-to-date nothing is printed."""
        rs_path, py_path = self._create_clean_package(tmp_path / "package")

        run_python([str(py_path)], tmp_path)  # run once to rebuild

        output, _ = run_python([str(py_path)], tmp_path)
        assert output == "get_num 10\nSUCCESS\n"

    def test_default_compile_error(self, tmp_path: Path) -> None:
        """If compilation fails then the error message from maturin is printed and an ImportError is raised."""
        rs_path, py_path = self._create_clean_package(tmp_path / "package")

        rs_path.write_text(rs_path.read_text().replace("10", ""))
        output, _ = run_python([str(py_path)], tmp_path, quiet=True)
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
        assert re.fullmatch(pattern, output, flags=re.MULTILINE | re.DOTALL) is not None

    def test_default_compile_warning(self, tmp_path: Path) -> None:
        """If compilation succeeds with warnings then the output of maturin is printed.
        If the module is already up to date but warnings were raised when it was first
        built, the warnings will be printed again.
        """
        rs_path, py_path = self._create_clean_package(tmp_path / "package")
        rs_path.write_text(rs_path.read_text().replace("10", "let x = 12; 20"))

        output1, _ = run_python([str(py_path)], tmp_path)
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
        assert (
            re.fullmatch(pattern, output1, flags=re.MULTILINE | re.DOTALL) is not None
        )

        output2, _ = run_python([str(py_path)], tmp_path)
        pattern = (
            'maturin.import_hook \\[WARNING\\] the last build of "my_script" succeeded with warnings:\n'
            ".*"
            "warning: unused variable: `x`"
            ".*"
            "get_num 20\n"
            "SUCCESS\n"
        )
        assert (
            re.fullmatch(pattern, output2, flags=re.MULTILINE | re.DOTALL) is not None
        )

    def test_reset_logger_without_configuring(self, tmp_path: Path) -> None:
        """If reset_logger is called then by default logging level INFO is not printed
        (because the messages are handled by the root logger).
        """
        rs_path, py_path = self._create_clean_package(tmp_path / "package")
        output, _ = run_python([str(py_path), "RESET_LOGGER"], tmp_path)
        assert output == "get_num 10\nSUCCESS\n"

    def test_successful_compilation_but_not_valid(self, tmp_path: Path) -> None:
        """If the script compiles but does not import correctly an ImportError is raised."""
        rs_path, py_path = self._create_clean_package(tmp_path / "package")
        rs_path.write_text(
            rs_path.read_text().replace("my_script", "my_script_new_name")
        )
        output, _ = run_python([str(py_path)], tmp_path, quiet=True)
        pattern = (
            'building "my_script"\n'
            'rebuilt and loaded module "my_script" in [0-9.]+s\n'
            "caught ImportError: dynamic module does not define module export function \\(PyInit_my_script\\)\n"
        )
        assert re.fullmatch(pattern, output, flags=re.MULTILINE) is not None
