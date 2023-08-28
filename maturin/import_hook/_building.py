import hashlib
import json
import logging
import os
import platform
import re
import shutil
import subprocess
import sys
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional, Tuple

from ._file_lock import FileLock
from ._logging import logger
from .settings import MaturinSettings


@dataclass
class BuildStatus:
    build_mtime: float
    source_path: Path
    maturin_args: List[str]
    maturin_output: str

    def to_json(self) -> dict:
        return {
            "build_mtime": self.build_mtime,
            "source_path": str(self.source_path),
            "maturin_args": self.maturin_args,
            "maturin_output": self.maturin_output,
        }

    @staticmethod
    def from_json(json_data: dict) -> Optional["BuildStatus"]:
        try:
            return BuildStatus(
                build_mtime=json_data["build_mtime"],
                source_path=Path(json_data["source_path"]),
                maturin_args=json_data["maturin_args"],
                maturin_output=json_data["maturin_output"],
            )
        except KeyError:
            logger.debug("failed to parse BuildStatus from %s", json_data)
            return None


class LockNotHeldError(Exception):
    pass


class BuildCache:
    def __init__(
        self, build_dir: Optional[Path], lock_timeout_seconds: Optional[float]
    ) -> None:
        self._build_dir = (
            build_dir if build_dir is not None else _get_default_build_dir()
        )
        self._lock = FileLock.new(
            self._build_dir / "lock", timeout_seconds=lock_timeout_seconds
        )

    @property
    def lock(self) -> FileLock:
        return self._lock

    def _build_status_path(self, source_path: Path) -> Path:
        if not self._lock.is_locked:
            raise LockNotHeldError
        path_hash = hashlib.sha1(bytes(source_path)).hexdigest()
        build_status_dir = self._build_dir / "build_status"
        build_status_dir.mkdir(parents=True, exist_ok=True)
        return build_status_dir / f"{path_hash}.json"

    def store_build_status(self, build_status: BuildStatus) -> None:
        with self._build_status_path(build_status.source_path).open("w") as f:
            json.dump(build_status.to_json(), f, indent="  ")

    def get_build_status(self, source_path: Path) -> Optional[BuildStatus]:
        try:
            with self._build_status_path(source_path).open("r") as f:
                return BuildStatus.from_json(json.load(f))
        except FileNotFoundError:
            return None

    def tmp_project_dir(self, project_path: Path, module_name: str) -> Path:
        if not self._lock.is_locked:
            raise LockNotHeldError
        path_hash = hashlib.sha1(bytes(project_path)).hexdigest()
        return self._build_dir / "project" / f"{module_name}_{path_hash}"


def _get_default_build_dir() -> Path:
    build_dir = os.environ.get("MATURIN_BUILD_DIR", None)
    if build_dir and os.access(sys.exec_prefix, os.W_OK):
        return Path(build_dir)
    elif os.access(sys.exec_prefix, os.W_OK):
        return Path(sys.exec_prefix) / "maturin_build_cache"
    else:
        version_string = sys.version.split()[0]
        interpreter_hash = hashlib.sha1(sys.exec_prefix.encode()).hexdigest()
        return (
            _get_cache_dir()
            / f"maturin_build_cache/{version_string}_{interpreter_hash}"
        )


def _get_cache_dir() -> Path:
    if os.name == "posix":
        if sys.platform == "darwin":
            return Path("~/Library/Caches").expanduser()
        else:
            xdg_cache_dir = os.environ.get("XDG_CACHE_HOME", None)
            return (
                Path(xdg_cache_dir) if xdg_cache_dir else Path("~/.cache").expanduser()
            )
    elif platform.platform().lower() == "windows":
        local_app_data = os.environ.get("LOCALAPPDATA", None)
        return (
            Path(local_app_data)
            if local_app_data
            else Path(r"~\AppData\Local").expanduser()
        )
    else:
        logger.warning("unknown OS. defaulting to ~/.cache as the cache directory")
        return Path("~/.cache").expanduser()


def generate_project_for_single_rust_file(
    build_dir: Path,
    rust_file: Path,
    available_features: Optional[list[str]],
) -> Path:
    project_dir = build_dir / rust_file.stem
    if project_dir.exists():
        shutil.rmtree(project_dir)

    success, output = _run_maturin(["new", "--bindings", "pyo3", str(project_dir)])
    if not success:
        msg = "Failed to generate project for rust file"
        raise ImportError(msg)

    if available_features is not None:
        available_features = [
            feature for feature in available_features if "/" not in feature
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


def build_wheel(
    manifest_path: Path,
    output_dir: Path,
    settings: MaturinSettings,
) -> str:
    success, output = _run_maturin(
        [
            "build",
            "--manifest-path",
            str(manifest_path),
            "--out",
            str(output_dir),
            *settings.to_args(),
        ],
    )
    if not success:
        msg = "Failed to build wheel with maturin"
        raise ImportError(msg)
    return output


def develop_build_project(
    manifest_path: Path,
    settings: MaturinSettings,
    skip_install: bool,
) -> str:
    args = ["develop", "--manifest-path", str(manifest_path)]
    if skip_install:
        args.append("--skip-install")
    args.extend(settings.to_args())
    success, output = _run_maturin(args)
    if not success:
        msg = "Failed to build package with maturin"
        raise ImportError(msg)
    return output


def _run_maturin(args: list[str]) -> Tuple[bool, str]:
    maturin_path = shutil.which("maturin")
    if maturin_path is None:
        msg = "maturin not found in the PATH"
        raise ImportError(msg)
    logger.debug('using maturin at: "%s"', maturin_path)

    command: List[str] = [maturin_path, *args]
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    output = result.stdout.decode()
    if result.returncode != 0:
        logger.error(
            f'command "{subprocess.list2cmdline(command)}" returned non-zero exit status: {result.returncode}'
        )
        logger.error("maturin output:\n%s", output)
        return False, output
    if logger.isEnabledFor(logging.DEBUG):
        logger.debug("maturin output:\n%s", output)
    return True, output


def build_unpacked_wheel(
    manifest_path: Path, output_dir: Path, settings: MaturinSettings
) -> str:
    if output_dir.exists():
        shutil.rmtree(output_dir)
    output = build_wheel(manifest_path, output_dir, settings)
    wheel_path = _find_single_file(output_dir, ".whl")
    if wheel_path is None:
        msg = "failed to generate wheel"
        raise ImportError(msg)
    with zipfile.ZipFile(wheel_path, "r") as f:
        f.extractall(output_dir)
    return output


def _find_single_file(dir_path: Path, extension: Optional[str]) -> Optional[Path]:
    if dir_path.exists():
        candidate_files = [
            p for p in dir_path.iterdir() if extension is None or p.suffix == extension
        ]
    else:
        candidate_files = []
    return candidate_files[0] if len(candidate_files) == 1 else None


def maturin_output_has_warnings(output: str) -> bool:
    return (
        re.search(r"warning: `.*` \((lib|bin)\) generated [0-9]+ warnings?", output)
        is not None
    )
