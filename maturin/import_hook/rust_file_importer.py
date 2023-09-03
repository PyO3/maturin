import contextlib
import importlib
import importlib.util
import logging
import math
import os
import shutil
import sys
import time
from importlib.machinery import ExtensionFileLoader, ModuleSpec
from pathlib import Path
from types import ModuleType
from typing import Optional, Sequence, Tuple, Union

from maturin.import_hook._building import (
    BuildCache,
    BuildStatus,
    LockedBuildCache,
    build_unpacked_wheel,
    maturin_output_has_warnings,
    run_maturin,
)
from maturin.import_hook._logging import logger
from maturin.import_hook._resolve_project import ProjectResolver, find_cargo_manifest
from maturin.import_hook.settings import MaturinSettings

__all__ = ["MaturinRustFileImporter", "install", "uninstall", "IMPORTER"]


class MaturinRustFileImporter(importlib.abc.MetaPathFinder):
    """An import hook for loading .rs files as though they were regular python modules."""

    def __init__(
        self,
        *,
        settings: Optional[MaturinSettings] = None,
        build_dir: Optional[Path] = None,
        force_rebuild: bool = False,
        lock_timeout_seconds: Optional[float] = 120,
        show_warnings: bool = True,
    ) -> None:
        self._force_rebuild = force_rebuild
        self._resolver = ProjectResolver()
        self._settings = settings
        self._build_cache = BuildCache(build_dir, lock_timeout_seconds)
        self._show_warnings = show_warnings

    def get_settings(self, module_path: str, source_path: Path) -> MaturinSettings:
        """This method can be overridden in subclasses to customize settings for specific projects."""
        return (
            self._settings if self._settings is not None else MaturinSettings.default()
        )

    @staticmethod
    def generate_project_for_single_rust_file(
        module_path: str,
        project_dir: Path,
        rust_file: Path,
        settings: MaturinSettings,
    ) -> Path:
        """This method can be overridden in subclasses to customize project generation."""
        if project_dir.exists():
            shutil.rmtree(project_dir)

        success, output = run_maturin(["new", "--bindings", "pyo3", str(project_dir)])
        if not success:
            msg = "Failed to generate project for rust file"
            raise ImportError(msg)

        if settings.features is not None:
            available_features = [
                feature for feature in settings.features if "/" not in feature
            ]
            cargo_manifest = project_dir / "Cargo.toml"
            cargo_manifest.write_text(
                "{}\n[features]\n{}".format(
                    cargo_manifest.read_text(),
                    "\n".join(f"{feature} = []" for feature in available_features),
                )
            )

        shutil.copy(rust_file, project_dir / "src/lib.rs")
        return project_dir

    def find_spec(
        self,
        fullname: str,
        path: Optional[Sequence[Union[str, bytes]]] = None,
        target: Optional[ModuleType] = None,
    ) -> Optional[ModuleSpec]:
        if fullname in sys.modules:
            return None

        start = time.perf_counter()

        if logger.isEnabledFor(logging.DEBUG):
            logger.debug('%s searching for "%s"', type(self).__name__, fullname)

        is_top_level_import = path is None
        if is_top_level_import:
            search_paths = [Path(p) for p in sys.path]
        else:
            assert path is not None
            search_paths = [Path(os.fsdecode(p)) for p in path]

        module_name = fullname.split(".")[-1]

        spec = None
        rebuilt = False
        for search_path in search_paths:
            single_rust_file_path = search_path / f"{module_name}.rs"
            if single_rust_file_path.is_file():
                spec, rebuilt = self._import_rust_file(
                    fullname, module_name, single_rust_file_path
                )
                if spec is not None:
                    break

        if spec is not None:
            duration = time.perf_counter() - start
            if rebuilt:
                logger.info(
                    'rebuilt and loaded module "%s" in %.3fs', fullname, duration
                )
            else:
                logger.debug('loaded module "%s" in %.3fs', fullname, duration)
        return spec

    def _import_rust_file(
        self, module_path: str, module_name: str, file_path: Path
    ) -> Tuple[Optional[ModuleSpec], bool]:
        logger.debug('importing rust file "%s" as "%s"', file_path, module_path)

        with self._build_cache.lock() as build_cache:
            output_dir = build_cache.tmp_project_dir(file_path, module_name)
            logger.debug("output dir: %s", output_dir)
            settings = self.get_settings(module_path, file_path)
            dist_dir = output_dir / "dist"
            package_dir = dist_dir / module_name

            spec, reason = self._get_spec_for_up_to_date_extension_module(
                package_dir, module_path, module_name, file_path, settings, build_cache
            )
            if spec is not None:
                return spec, False
            logger.debug('module "%s" will be rebuilt because: %s', module_path, reason)

            logger.info('building "%s"', module_path)
            logger.debug('creating project for "%s" and compiling', file_path)
            start = time.perf_counter()
            project_dir = self.generate_project_for_single_rust_file(
                module_path, output_dir / file_path.stem, file_path, settings
            )
            manifest_path = find_cargo_manifest(project_dir)
            if manifest_path is None:
                msg = (
                    f"cargo manifest not found in the project generated for {file_path}"
                )
                raise ImportError(msg)

            maturin_output = build_unpacked_wheel(manifest_path, dist_dir, settings)
            logger.debug(
                'compiled "%s" in %.3fs',
                file_path,
                time.perf_counter() - start,
            )

            if self._show_warnings and maturin_output_has_warnings(maturin_output):
                self._log_build_warnings(module_path, maturin_output, is_fresh=True)
            extension_module_path = _find_extension_module(
                dist_dir / module_name, module_name, require=True
            )
            if extension_module_path is None:
                logger.error(
                    'cannot find extension module for "%s" after rebuild', module_path
                )
                return None, True
            build_status = BuildStatus(
                extension_module_path.stat().st_mtime,
                file_path,
                settings.to_args(),
                maturin_output,
            )
            build_cache.store_build_status(build_status)
            return (
                _get_spec_for_extension_module(module_path, extension_module_path),
                True,
            )

    def _get_spec_for_up_to_date_extension_module(
        self,
        search_dir: Path,
        module_path: str,
        module_name: str,
        source_path: Path,
        settings: MaturinSettings,
        build_cache: LockedBuildCache,
    ) -> Tuple[Optional[ModuleSpec], Optional[str]]:
        """Return a spec for the given module at the given search_dir if it exists and is newer than the source
        code that it is derived from.
        """
        logger.debug('checking whether the module "%s" is up to date', module_path)

        if self._force_rebuild:
            return None, "forcing rebuild"
        extension_module_path = _find_extension_module(
            search_dir, module_name, require=False
        )
        if extension_module_path is None:
            return None, "already built module not found"

        extension_module_mtime = extension_module_path.stat().st_mtime
        if extension_module_mtime < source_path.stat().st_mtime:
            return None, "module is out of date"

        build_status = build_cache.get_build_status(source_path)
        if build_status is None:
            return None, "no build status found"
        if build_status.source_path != source_path:
            return None, "source path in build status does not match the project dir"
        if not math.isclose(build_status.build_mtime, extension_module_mtime):
            return None, "installed package mtime does not match build status mtime"
        if build_status.maturin_args != settings.to_args():
            return None, "current maturin args do not match the previous build"

        spec = _get_spec_for_extension_module(module_path, extension_module_path)
        if spec is None:
            return None, "module not found"

        logger.debug('module up to date: "%s" (%s)', module_path, spec.origin)

        if self._show_warnings and maturin_output_has_warnings(
            build_status.maturin_output
        ):
            self._log_build_warnings(
                module_path, build_status.maturin_output, is_fresh=False
            )

        return spec, None

    def _log_build_warnings(
        self, module_path: str, maturin_output: str, is_fresh: bool
    ) -> None:
        prefix = "" if is_fresh else "the last "
        message = '%sbuild of "%s" succeeded with warnings:\n%s'
        if self._show_warnings:
            logger.warning(message, prefix, module_path, maturin_output)
        else:
            logger.debug(message, prefix, module_path, maturin_output)


def _find_extension_module(
    dir_path: Path, module_name: str, *, require: bool = False
) -> Optional[Path]:
    # the suffixes include the platform tag and file extension eg '.cpython-311-x86_64-linux-gnu.so'
    for suffix in importlib.machinery.EXTENSION_SUFFIXES:
        extension_path = dir_path / f"{module_name}{suffix}"
        if extension_path.exists():
            return extension_path
    if require:
        msg = f'could not find module "{module_name}" in "{dir_path}"'
        raise ImportError(msg)
    return None


def _get_spec_for_extension_module(
    module_path: str, extension_module_path: Path
) -> Optional[ModuleSpec]:
    return importlib.util.spec_from_loader(
        module_path, ExtensionFileLoader(module_path, str(extension_module_path))
    )


IMPORTER: Optional[MaturinRustFileImporter] = None


def install(
    *,
    settings: Optional[MaturinSettings] = None,
    build_dir: Optional[Path] = None,
    force_rebuild: bool = False,
    lock_timeout_seconds: Optional[float] = 120,
    show_warnings: bool = True,
) -> MaturinRustFileImporter:
    """Install the 'rust file' importer to import .rs files as though
    they were regular python modules.

    :param settings: settings corresponding to flags passed to maturin.

    :param build_dir: where to put the compiled artifacts. defaults to `$MATURIN_BUILD_DIR`,
        `sys.exec_prefix / 'maturin_build_cache'` or
        `$HOME/.cache/maturin_build_cache/<interpreter_hash>` in order of preference

    :param force_rebuild: whether to always rebuild and skip checking whether anything has changed

    :param lock_timeout_seconds: a lock is required to prevent projects from being built concurrently.
        If the lock is not released before this timeout is reached the import hook stops waiting and aborts

    :param show_warnings: whether to show compilation warnings

    """
    global IMPORTER
    if IMPORTER is not None:
        with contextlib.suppress(ValueError):
            sys.meta_path.remove(IMPORTER)
    IMPORTER = MaturinRustFileImporter(
        settings=settings,
        build_dir=build_dir,
        force_rebuild=force_rebuild,
        lock_timeout_seconds=lock_timeout_seconds,
        show_warnings=show_warnings,
    )
    sys.meta_path.insert(0, IMPORTER)
    return IMPORTER


def uninstall() -> None:
    """Uninstall the rust file importer import hook."""
    global IMPORTER
    if IMPORTER is not None:
        with contextlib.suppress(ValueError):
            sys.meta_path.remove(IMPORTER)
        IMPORTER = None
