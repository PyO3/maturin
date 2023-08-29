from pathlib import Path
from typing import Optional, Set

from maturin.import_hook import project_importer, rust_file_importer
from maturin.import_hook._logging import reset_logger
from maturin.import_hook.settings import MaturinSettings

__all__ = ["install", "uninstall", "reset_logger"]


def install(
    *,
    enable_project_importer: bool = True,
    enable_rs_file_importer: bool = True,
    settings: Optional[MaturinSettings] = None,
    build_dir: Optional[Path] = None,
    install_new_packages: bool = True,
    force_rebuild: bool = False,
    excluded_dir_names: Optional[Set[str]] = None,
    lock_timeout_seconds: Optional[float] = 120,
    show_warnings: bool = True,
) -> None:
    """Install import hooks for automatically rebuilding and importing maturin projects or .rs files.

    :param enable_project_importer: enable the hook for automatically rebuilding editable installed maturin projects

    :param enable_rs_file_importer: enable the hook for importing .rs files as though they were regular python modules

    :param settings: settings corresponding to flags passed to maturin.

    :param build_dir: where to put the compiled artifacts. defaults to `$MATURIN_BUILD_DIR`,
        `sys.exec_prefix / 'maturin_build_cache'` or
        `$HOME/.cache/maturin_build_cache/<interpreter_hash>` in order of preference

    :param install_new_packages: whether to install detected packages using the import hook even if they
        are not already installed into the virtual environment or are installed in non-editable mode.

    :param force_rebuild: whether to always rebuild and skip checking whether anything has changed

    :param excluded_dir_names: directory names to exclude when determining whether a project has changed
        and so whether the extension module needs to be rebuilt

    :param lock_timeout_seconds: a lock is required to prevent projects from being built concurrently.
        If the lock is not released before this timeout is reached the import hook stops waiting and aborts

    :param show_warnings: whether to show compilation warnings

    """
    if enable_rs_file_importer:
        rust_file_importer.install(
            settings=settings,
            build_dir=build_dir,
            force_rebuild=force_rebuild,
            lock_timeout_seconds=lock_timeout_seconds,
            show_warnings=show_warnings,
        )
    if enable_project_importer:
        project_importer.install(
            settings=settings,
            build_dir=build_dir,
            install_new_packages=install_new_packages,
            force_rebuild=force_rebuild,
            excluded_dir_names=excluded_dir_names,
            lock_timeout_seconds=lock_timeout_seconds,
            show_warnings=show_warnings,
        )


def uninstall() -> None:
    """Remove the import hooks."""
    project_importer.uninstall()
    rust_file_importer.uninstall()
