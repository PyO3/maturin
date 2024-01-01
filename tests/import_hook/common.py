import itertools
import multiprocessing
import os
import re
import shutil
import site
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, List, Optional, Tuple, Callable, Any, Dict

from maturin.import_hook.project_importer import _fix_direct_url, _load_dist_info

verbose = True


script_dir = Path(__file__).resolve().parent
maturin_dir = script_dir.parent.parent
test_crates = maturin_dir / "test-crates"

IMPORT_HOOK_HEADER = """
import logging
logging.basicConfig(format='%(name)s [%(levelname)s] %(message)s', level=logging.DEBUG)

from maturin import import_hook
import_hook.reset_logger()
import_hook.install()
"""


EXCLUDED_PROJECTS = {
    "hello-world",  # not imported as a python module (subprocess only)
    "license-test",  # not imported as a python module (subprocess only)
    "pyo3-bin",  # not imported as a python module (subprocess only)
}


def with_underscores(project_name: str) -> str:
    return project_name.replace("-", "_")


def all_test_crate_names() -> List[str]:
    return sorted(
        p.name
        for p in test_crates.iterdir()
        if (p / "check_installed/check_installed.py").exists() and (p / "pyproject.toml").exists()
        if p.name not in EXCLUDED_PROJECTS
    )


def mixed_test_crate_names() -> List[str]:
    return [name for name in all_test_crate_names() if "mixed" in name]


def run_python(
    args: List[str],
    cwd: Path,
    *,
    python_path: Optional[List[Path]] = None,
    quiet: bool = False,
    expect_error: bool = False,
) -> Tuple[str, float]:
    start = time.perf_counter()

    env = os.environ
    if python_path is not None:
        env["PYTHONPATH"] = os.pathsep.join(str(p) for p in itertools.chain(python_path, [maturin_dir]))
    else:
        env["PYTHONPATH"] = str(maturin_dir)

    cmd = [sys.executable, *args]
    try:
        proc = subprocess.run(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=True,
            cwd=cwd,
            env=env,
        )
        output = proc.stdout.decode()
    except subprocess.CalledProcessError as e:
        output = e.stdout.decode()
        if verbose and not quiet and not expect_error:
            message = "\n".join(
                [
                    "-" * 40,
                    "ERROR:",
                    subprocess.list2cmdline(cmd),
                    "",
                    output,
                    "-" * 40,
                ]
            )
            print(message, file=sys.stderr)
        if not expect_error:
            raise
    duration = time.perf_counter() - start

    output = output.replace("\r\n", "\n")

    if verbose and not quiet:
        print("-" * 40)
        print(subprocess.list2cmdline(cmd))
        print(output)
        print("-" * 40)

    return output, duration


def run_python_code(
    python_script: str,
    *,
    args: Optional[List[str]] = None,
    cwd: Optional[Path] = None,
    python_path: Optional[List[Path]] = None,
    quiet: bool = False,
    expect_error: bool = False,
) -> Tuple[str, float]:
    with tempfile.TemporaryDirectory("run_python_code") as tmpdir_str:
        tmpdir = Path(tmpdir_str)
        tmp_script_path = tmpdir / "script.py"
        tmp_script_path.write_text(python_script)

        python_args = [str(tmp_script_path)]
        if args is not None:
            python_args.extend(args)

        return run_python(
            python_args,
            cwd=cwd or tmpdir,
            python_path=python_path,
            quiet=quiet,
            expect_error=expect_error,
        )


def log(message: str) -> None:
    if verbose:
        print(message)


def uninstall(project_name: str) -> None:
    log(f"uninstalling {project_name}")
    subprocess.check_call([sys.executable, "-m", "pip", "uninstall", "-y", project_name])


def install_editable(project_dir: Path) -> None:
    """Install the given project to the virtualenv in editable mode."""
    log(f"installing {project_dir.name} in editable/unpacked mode")
    env = os.environ.copy()
    env["VIRTUAL_ENV"] = sys.exec_prefix
    subprocess.check_call(["maturin", "develop"], cwd=project_dir, env=env)
    package_name = with_underscores(project_dir.name)
    _fix_direct_url(project_dir, package_name)


def install_non_editable(project_dir: Path) -> None:
    log(f"installing {project_dir.name} in non-editable mode")
    subprocess.check_call([sys.executable, "-m", "pip", "install", str(project_dir)])


def _is_installed_as_pth(project_name: str) -> bool:
    package_name = with_underscores(project_name)
    return any((Path(path) / f"{package_name}.pth").exists() for path in site.getsitepackages())


def _is_installed_editable_with_direct_url(project_name: str, project_dir: Path) -> bool:
    package_name = with_underscores(project_name)
    for path in site.getsitepackages():
        linked_path, is_editable = _load_dist_info(Path(path), package_name)
        if linked_path == project_dir:
            if not is_editable:
                log(f'project "{project_name}" is installed but not in editable mode')
            return is_editable
        else:
            log(f'found linked path "{linked_path}" for project "{project_name}". Expected "{project_dir}"')
    return False


def is_installed_correctly(project_name: str, project_dir: Path, is_mixed: bool) -> bool:
    installed_as_pth = _is_installed_as_pth(project_name)
    installed_editable_with_direct_url = _is_installed_editable_with_direct_url(project_name, project_dir)
    log(
        f"checking if {project_name} is installed correctly. "
        f"{is_mixed=}, {installed_as_pth=} {installed_editable_with_direct_url=}"
    )
    return installed_editable_with_direct_url and (installed_as_pth == is_mixed)


def get_project_copy(project_dir: Path, output_path: Path) -> Path:
    for relative_path in _get_relative_files_tracked_by_git(project_dir):
        output_file_path = output_path / relative_path
        output_file_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(project_dir / relative_path, output_file_path)
    return output_path


def _get_relative_files_tracked_by_git(root: Path) -> Iterable[Path]:
    """This is used to ignore built artifacts to create a clean copy."""
    output = subprocess.check_output(["git", "ls-tree", "--name-only", "-z", "-r", "HEAD"], cwd=root)
    for relative_path_bytes in output.split(b"\x00"):
        relative_path = Path(os.fsdecode(relative_path_bytes))
        if (root / relative_path).is_file():
            yield relative_path


def create_project_from_blank_template(project_name: str, output_path: Path, *, mixed: bool) -> Path:
    project_dir = get_project_copy(script_dir / "blank-project", output_path)
    project_name = project_name.replace("_", "-")
    package_name = project_name.replace("-", "_")
    for path in [
        project_dir / "pyproject.toml",
        project_dir / "Cargo.toml",
        project_dir / "src/lib.rs",
    ]:
        path.write_text(path.read_text().replace("blank-project", project_name).replace("blank_project", package_name))
    if mixed:
        (project_dir / package_name).mkdir()
        (project_dir / package_name / "__init__.py").write_text(f"from .{package_name} import *")
    return project_dir


def remove_ansii_escape_characters(text: str) -> str:
    """Remove escape characters (eg used to color terminal output) from the given string.

    based on: https://stackoverflow.com/a/14693789
    """
    return re.sub(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])", "", text)


def check_match(text: str, pattern: str, *, flags: int = 0) -> None:
    assert (
        re.fullmatch(pattern, text, flags=flags) is not None
    ), f'text does not match pattern:\npattern: "{pattern}"\ntext:\n{text}'


@dataclass
class PythonProcessOutput:
    output: str
    duration: Optional[float]
    success: bool


def run_concurrent_python(
    num: int, func: Callable[..., Tuple[str, float]], args: Dict[str, Any]
) -> List[PythonProcessOutput]:
    outputs: List[PythonProcessOutput] = []
    with multiprocessing.Pool(processes=num) as pool:
        processes = []
        for i in range(num):
            processes.append(pool.apply_async(func, kwds=args))

        for i, proc in enumerate(processes):
            try:
                output, duration = proc.get()
            except subprocess.CalledProcessError as e:
                stdout = "None" if e.stdout is None else e.stdout.decode()
                stderr = "None" if e.stderr is None else e.stderr.decode()
                output = "\n".join(["-" * 50, "Stdout:", stdout, "Stderr:", stderr, "-" * 50])
                success = False
                duration = None
            else:
                success = True
            outputs.append(PythonProcessOutput(output, duration, success))

        for i, o in enumerate(outputs):
            log(f"# Subprocess {i}")
            log(f"success: {o.success}")
            log(f"duration: {o.duration}")
            log(f"output:\n{o.output}")

    return outputs
