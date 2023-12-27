import itertools
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from maturin.import_hook._logging import logger

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib  # type: ignore


def find_cargo_manifest(project_dir: Path) -> Optional[Path]:
    pyproject_path = project_dir / "pyproject.toml"
    if pyproject_path.exists():
        with pyproject_path.open("rb") as f:
            pyproject = tomllib.load(f)
        relative_manifest_path = pyproject.get("tool", {}).get("maturin", {}).get("manifest-path", None)
        if relative_manifest_path is not None:
            return project_dir / relative_manifest_path

    manifest_path = project_dir / "Cargo.toml"
    if manifest_path.exists():
        return manifest_path
    manifest_path = project_dir / "rust/Cargo.toml"
    if manifest_path.exists():
        return manifest_path
    return None


def is_maybe_maturin_project(project_dir: Path) -> bool:
    """note: this function does not check if this really is a maturin project for simplicity."""
    return (project_dir / "pyproject.toml").exists() and find_cargo_manifest(project_dir) is not None


class ProjectResolver:
    def __init__(self) -> None:
        self._resolved_project_cache: Dict[Path, Optional[MaturinProject]] = {}

    def resolve(self, project_dir: Path) -> Optional["MaturinProject"]:
        if project_dir not in self._resolved_project_cache:
            resolved = None
            try:
                resolved = _resolve_project(project_dir)
            except ProjectResolveError as e:
                logger.info('failed to resolve project "%s": %s', project_dir, e)
            self._resolved_project_cache[project_dir] = resolved
        else:
            resolved = self._resolved_project_cache[project_dir]
        return resolved


@dataclass
class MaturinProject:
    cargo_manifest_path: Path
    # the name of the compiled extension module without any suffix
    # (i.e. "some_package.my_module" instead of "some_package/my_module.cpython-311-x86_64-linux-gnu")
    module_full_name: str
    # the root of the python part of the project (or the project root if there is none)
    python_dir: Path
    # the path to the top level python package if the project is mixed
    python_module: Optional[Path]
    # the location that the compiled extension module is written to when installed in editable/unpacked mode
    extension_module_dir: Optional[Path]
    # path dependencies listed in the Cargo.toml of the main project
    immediate_path_dependencies: List[Path]
    # all path dependencies including transitive dependencies
    _all_path_dependencies: Optional[List[Path]] = None

    @property
    def package_name(self) -> str:
        return self.module_full_name.split(".")[0]

    @property
    def module_name(self) -> str:
        return self.module_full_name.split(".")[-1]

    @property
    def is_mixed(self) -> bool:
        """Whether the project contains both python and rust code."""
        return self.extension_module_dir is not None

    @property
    def all_path_dependencies(self) -> List[Path]:
        if self._all_path_dependencies is None:
            self._all_path_dependencies = _find_all_path_dependencies(self.immediate_path_dependencies)
        return self._all_path_dependencies


def _find_all_path_dependencies(immediate_path_dependencies: List[Path]) -> List[Path]:
    if not immediate_path_dependencies:
        return []
    all_path_dependencies = set()
    to_search = immediate_path_dependencies.copy()
    while to_search:
        dependency_project_dir = to_search.pop()
        if dependency_project_dir in all_path_dependencies:
            continue
        all_path_dependencies.add(dependency_project_dir)
        manifest_path = dependency_project_dir / "Cargo.toml"
        if manifest_path.exists():
            with manifest_path.open("rb") as f:
                cargo = tomllib.load(f)
            to_search.extend(_get_immediate_path_dependencies(dependency_project_dir, cargo))
    return sorted(all_path_dependencies)


class ProjectResolveError(Exception):
    pass


def _resolve_project(project_dir: Path) -> MaturinProject:
    """This follows the same logic as project_layout.rs.

    module_writer::write_bindings_module() is the function that copies the extension file to `rust_module / so_filename`
    """
    pyproject_path = project_dir / "pyproject.toml"
    if not pyproject_path.exists():
        msg = "no pyproject.toml found"
        raise ProjectResolveError(msg)
    with pyproject_path.open("rb") as f:
        pyproject = tomllib.load(f)

    manifest_path = find_cargo_manifest(project_dir)
    if manifest_path is None:
        msg = "no Cargo.toml found"
        raise ProjectResolveError(msg)
    with manifest_path.open("rb") as f:
        cargo = tomllib.load(f)

    module_full_name = _resolve_module_name(pyproject, cargo)
    if module_full_name is None:
        msg = "could not resolve module_full_name"
        raise ProjectResolveError(msg)

    python_dir = _resolve_py_root(project_dir, pyproject)

    extension_module_dir: Optional[Path]
    python_module: Optional[Path]
    python_module, extension_module_dir, extension_module_name = _resolve_rust_module(python_dir, module_full_name)
    immediate_path_dependencies = _get_immediate_path_dependencies(manifest_path.parent, cargo)

    if not python_module.exists():
        extension_module_dir = None
        python_module = None

    return MaturinProject(
        cargo_manifest_path=manifest_path,
        module_full_name=module_full_name,
        python_dir=python_dir,
        python_module=python_module,
        extension_module_dir=extension_module_dir,
        immediate_path_dependencies=immediate_path_dependencies,
    )


def _resolve_rust_module(python_dir: Path, module_name: str) -> Tuple[Path, Path, str]:
    """This follows the same logic as project_layout.rs (ProjectLayout::determine).

    rust_module is the directory that the extension library gets written to when the package is
    installed in editable mode
    """
    parts = module_name.split(".")
    if len(parts) > 1:
        python_module = python_dir / parts[0]
        extension_module_dir = python_dir / Path(*parts[:-1])
        extension_module_name = parts[-1]
    else:
        python_module = python_dir / module_name
        extension_module_dir = python_dir / module_name
        extension_module_name = module_name
    return python_module, extension_module_dir, extension_module_name


def _resolve_module_name(pyproject: Dict[str, Any], cargo: Dict[str, Any]) -> Optional[str]:
    """This follows the same logic as project_layout.rs (ProjectResolver::resolve).

    Precedence:
     * Explicitly declared pyproject.toml `tool.maturin.module-name`
     * Cargo.toml `lib.name`
     * pyproject.toml `project.name`
     * Cargo.toml `package.name`

    """
    module_name = pyproject.get("tool", {}).get("maturin", {}).get("module-name", None)
    if module_name is not None:
        return module_name
    module_name = cargo.get("lib", {}).get("name", None)
    if module_name is not None:
        return module_name
    module_name = pyproject.get("project", {}).get("name", None)
    if module_name is not None:
        return module_name
    return cargo.get("package", {}).get("name", None)


def _get_immediate_path_dependencies(manifest_dir_path: Path, cargo: Dict[str, Any]) -> List[Path]:
    path_dependencies = []
    for dependency in cargo.get("dependencies", {}).values():
        if isinstance(dependency, dict):
            relative_path = dependency.get("path", None)
            if relative_path is not None:
                path_dependencies.append((manifest_dir_path / relative_path).resolve())
    return path_dependencies


def _resolve_py_root(project_dir: Path, pyproject: Dict[str, Any]) -> Path:
    """This follows the same logic as project_layout.rs."""
    py_root = pyproject.get("tool", {}).get("maturin", {}).get("python-source", None)
    if py_root is not None:
        return project_dir / py_root
    project_name = pyproject.get("project", {}).get("name", None)
    if project_name is None:
        return project_dir

    rust_cargo_toml_found = (project_dir / "rust/Cargo.toml").exists()

    python_packages = pyproject.get("tool", {}).get("maturin", {}).get("python-packages", [])

    package_name = project_name.replace("-", "_")
    python_src_found = any(
        (project_dir / p / "__init__.py").is_file() for p in itertools.chain((f"src/{package_name}/",), python_packages)
    )
    if rust_cargo_toml_found and python_src_found:
        return project_dir / "src"
    else:
        return project_dir
