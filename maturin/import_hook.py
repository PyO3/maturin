import contextlib
import importlib
import importlib.util
from importlib import abc
from importlib.machinery import ModuleSpec
import os
import pathlib
import shutil
import sys
import subprocess
from typing import Optional

import toml


class Importer(abc.MetaPathFinder):
    """A meta-path importer for the maturin based packages"""

    def __init__(self, bindings: Optional[str] = None, release: bool = False):
        self.bindings = bindings
        self.release = release

    def find_spec(self, fullname, path, target=None):
        if fullname in sys.modules:
            return
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

    def _build_and_load(self, fullname: str, cargo_toml: pathlib.Path) -> ModuleSpec:
        build_module(cargo_toml, bindings=self.bindings)
        loader = Loader(fullname)
        return importlib.util.spec_from_loader(fullname, loader)


class Loader(abc.Loader):
    def __init__(self, fullname):
        self.fullname = fullname

    def load_module(self, fullname):
        return importlib.import_module(self.fullname)


def _is_cargo_project(cargo_toml: pathlib.Path, module_name: str) -> bool:
    with contextlib.suppress(FileNotFoundError):
        with open(cargo_toml) as f:
            cargo = toml.load(f)
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

    command = ["maturin", "new", "-b", bindings, project_dir]
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
    manifest_path: pathlib.Path, bindings: Optional[str] = None, release: bool = False
):
    command = ["maturin", "develop", "-m", manifest_path]
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


def install(bindings: Optional[str] = None, release: bool = False):
    """
    Install the import hook.

    :param bindings: Which kind of bindings to use.
        Possible values are pyo3, rust-cpython and cffi

    :param release: Build in release mode, otherwise debug mode by default
    """
    if _have_importer():
        return
    importer = Importer(bindings=bindings, release=release)
    sys.meta_path.append(importer)
    return importer


def uninstall(importer: Importer):
    """
    Uninstall the import hook.
    """
    try:
        sys.meta_path.remove(importer)
    except ValueError:
        pass
