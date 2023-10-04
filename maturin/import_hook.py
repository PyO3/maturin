from __future__ import annotations

import contextlib
import importlib
import importlib.util
import os
import pathlib
import shutil
import subprocess
import sys
from contextvars import ContextVar
from importlib import abc
from importlib.machinery import ModuleSpec
from types import ModuleType
from typing import Sequence

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib  # type: ignore


# Track if we have already built the package, so we can avoid infinite
# recursion.
_ALREADY_BUILT = ContextVar("_ALREADY_BUILT", default=False)


class Importer(abc.MetaPathFinder):
    """A meta-path importer for the maturin based packages"""

    def __init__(self, bindings: str | None = None, release: bool = False):
        self.bindings = bindings
        self.release = release

    def find_spec(
        self,
        fullname: str,
        path: Sequence[str | bytes] | None = None,
        target: ModuleType | None = None,
    ) -> ModuleSpec | None:
        if fullname in sys.modules:
            return None
        if _ALREADY_BUILT.get():
            # At this point we'll just import normally.
            return None

        mod_parts = fullname.split(".")
        module_name = mod_parts[-1]

        cwd = pathlib.Path(os.getcwd())
        # Full Cargo project in cwd
        cargo_toml = cwd / "Cargo.toml"
        if _is_cargo_project(cargo_toml, module_name):
            return self._build_and_load(fullname, cargo_toml)

        # Full Cargo project in subdirectory of cwd
        cargo_toml = cwd / module_name / "Cargo.toml"
        if _is_cargo_project(cargo_toml, module_name):
            return self._build_and_load(fullname, cargo_toml)
        # module name with '-' instead of '_'
        cargo_toml = cwd / module_name.replace("_", "-") / "Cargo.toml"
        if _is_cargo_project(cargo_toml, module_name):
            return self._build_and_load(fullname, cargo_toml)

        # Single .rs file
        rust_file = cwd / (module_name + ".rs")
        if rust_file.exists():
            project_dir = generate_project(rust_file, bindings=self.bindings or "pyo3")
            cargo_toml = project_dir / "Cargo.toml"
            return self._build_and_load(fullname, cargo_toml)

        return None

    def _build_and_load(
        self, fullname: str, cargo_toml: pathlib.Path
    ) -> ModuleSpec | None:
        build_module(cargo_toml, bindings=self.bindings)
        loader = Loader(fullname)
        return importlib.util.spec_from_loader(fullname, loader)


class Loader(abc.Loader):
    def __init__(self, fullname: str):
        self.fullname = fullname

    def load_module(self, fullname: str) -> ModuleType:
        # By the time we're loading, the package should've already been built
        # by the previous step of finding the spec.
        old_value = _ALREADY_BUILT.set(True)
        try:
            return importlib.import_module(self.fullname)
        finally:
            _ALREADY_BUILT.reset(old_value)


def _is_cargo_project(cargo_toml: pathlib.Path, module_name: str) -> bool:
    with contextlib.suppress(FileNotFoundError):
        with open(cargo_toml, "rb") as f:
            cargo = tomllib.load(f)
            package_name = cargo.get("package", {}).get("name")
            if (
                package_name == module_name
                or package_name.replace("-", "_") == module_name
            ):
                return True
    return False


def generate_project(rust_file: pathlib.Path, bindings: str = "pyo3") -> pathlib.Path:
    build_dir = pathlib.Path(os.getcwd()) / "build"
    project_dir = build_dir / rust_file.stem
    if project_dir.exists():
        shutil.rmtree(project_dir)

    command: list[str] = ["maturin", "new", "-b", bindings, str(project_dir)]
    result = subprocess.run(command, stdout=subprocess.PIPE)
    if result.returncode != 0:
        sys.stderr.write(
            f"Error: command {command} returned non-zero exit status {result.returncode}\n"
        )
        raise ImportError("Failed to generate cargo project")

    with open(rust_file) as f:
        lib_rs_content = f.read()
    lib_rs = project_dir / "src" / "lib.rs"
    with open(lib_rs, "w") as f:
        f.write(lib_rs_content)
    return project_dir


def build_module(
    manifest_path: pathlib.Path, bindings: str | None = None, release: bool = False
) -> None:
    command = ["maturin", "develop", "-m", str(manifest_path)]
    if bindings:
        command.append("-b")
        command.append(bindings)
    if release:
        command.append("--release")
    result = subprocess.run(command, stdout=subprocess.PIPE)
    sys.stdout.buffer.write(result.stdout)
    sys.stdout.flush()
    if result.returncode != 0:
        sys.stderr.write(
            f"Error: command {command} returned non-zero exit status {result.returncode}\n"
        )
        raise ImportError("Failed to build module with maturin")


def _have_importer() -> bool:
    for importer in sys.meta_path:
        if isinstance(importer, Importer):
            return True
    return False


def install(bindings: str | None = None, release: bool = False) -> Importer | None:
    """
    Install the import hook.

    :param bindings: Which kind of bindings to use.
        Possible values are pyo3, rust-cpython and cffi

    :param release: Build in release mode, otherwise debug mode by default
    """
    if _have_importer():
        return None
    importer = Importer(bindings=bindings, release=release)
    sys.meta_path.insert(0, importer)
    return importer


def uninstall(importer: Importer) -> None:
    """
    Uninstall the import hook.
    """
    try:
        sys.meta_path.remove(importer)
    except ValueError:
        pass
