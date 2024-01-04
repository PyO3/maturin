import json
import os
import platform
import shutil
import time
from operator import itemgetter
from pathlib import Path
from typing import Any, Dict, List, Optional

import pytest

from maturin.import_hook import MaturinSettings, reset_logger
from maturin.import_hook._building import BuildCache, BuildStatus
from maturin.import_hook._resolve_project import ProjectResolveError, _resolve_project
from maturin.import_hook.project_importer import (
    _get_installed_package_mtime,
    _get_project_mtime,
    _load_dist_info,
    _uri_to_path,
)
from maturin.import_hook.settings import MaturinBuildSettings, MaturinDevelopSettings

from .common import log, test_crates

# set this to be able to run these tests without going through run.rs each time
SAVED_RESOLVED_PACKAGES_PATH: Optional[Path] = None

if SAVED_RESOLVED_PACKAGES_PATH is not None:
    if "RESOLVED_PACKAGES_PATH" in os.environ:
        shutil.copy(os.environ["RESOLVED_PACKAGES_PATH"], SAVED_RESOLVED_PACKAGES_PATH)
    os.environ["RESOLVED_PACKAGES_PATH"] = str(SAVED_RESOLVED_PACKAGES_PATH)


reset_logger()


def test_settings() -> None:
    assert MaturinSettings().to_args() == []
    assert MaturinBuildSettings().to_args() == []
    assert MaturinDevelopSettings().to_args() == []

    settings = MaturinSettings(
        release=True,
        strip=True,
        quiet=True,
        jobs=1,
        profile="profile1",
        features=["feature1", "feature2"],
        all_features=True,
        no_default_features=True,
        target="target1",
        ignore_rust_version=True,
        color=True,
        frozen=True,
        locked=True,
        offline=True,
        config={"key1": "value1", "key2": "value2"},
        unstable_flags=["unstable1", "unstable2"],
        verbose=2,
        rustc_flags=["flag1", "flag2"],
    )
    # fmt: off
    assert settings.to_args() == [
        "--release",
        "--strip",
        "--quiet",
        "--jobs", "1",
        "--profile", "profile1",
        "--features", "feature1,feature2",
        "--all-features",
        "--no-default-features",
        "--target", "target1",
        "--ignore-rust-version",
        "--color", "always",
        "--frozen",
        "--locked",
        "--offline",
        "--config", "key1=value1",
        "--config", "key2=value2",
        "-Z", "unstable1",
        "-Z", "unstable2",
        "-vv",
        "flag1",
        "flag2",
    ]
    # fmt: on

    build_settings = MaturinBuildSettings(skip_auditwheel=True, zig=True, color=False, rustc_flags=["flag1", "flag2"])
    assert build_settings.to_args() == [
        "--skip-auditwheel",
        "--zig",
        "--color",
        "never",
        "flag1",
        "flag2",
    ]

    develop_settings = MaturinDevelopSettings(
        extras=["extra1", "extra2"],
        skip_install=True,
        color=False,
        rustc_flags=["flag1", "flag2"],
    )
    assert develop_settings.to_args() == [
        "--extras",
        "extra1,extra2",
        "--skip-install",
        "--color",
        "never",
        "flag1",
        "flag2",
    ]


class TestGetProjectMtime:
    def test_missing_extension(self, tmp_path: Path) -> None:
        assert _get_project_mtime(tmp_path, [], tmp_path / "missing", set()) is None
        extension_dir = tmp_path / "extension"
        extension_dir.mkdir()
        assert _get_project_mtime(tmp_path, [], extension_dir, set()) is None

    def test_missing_path_dep(self, tmp_path: Path) -> None:
        (tmp_path / "extension").touch()
        project_mtime = _get_project_mtime(tmp_path, [tmp_path / "missing"], tmp_path / "extension", set())
        assert project_mtime is None

    def test_simple(self, tmp_path: Path) -> None:
        src_dir = tmp_path / "src"
        src_dir.mkdir()
        (src_dir / "source_file.rs").touch()
        _small_sleep()
        (tmp_path / "extension_module").touch()
        project_mtime = _get_project_mtime(tmp_path, [], tmp_path / "extension_module", set())
        assert project_mtime == (tmp_path / "extension_module").stat().st_mtime

        (tmp_path / "extension_module").unlink()
        (tmp_path / "extension_module").mkdir()
        (tmp_path / "extension_module/stuff").touch()

        # if the extension module is a directory then it should be excluded from the project mtime
        # calculation as it may contain pycache files that are generated after installation
        project_mtime = _get_project_mtime(tmp_path, [], tmp_path / "extension_module", set())
        assert project_mtime == (src_dir / "source_file.rs").stat().st_mtime

        project_mtime = _get_project_mtime(tmp_path, [], tmp_path / "extension_module", {"src"})
        assert project_mtime is None

    def test_simple_path_dep(self, tmp_path: Path) -> None:
        project_a = tmp_path / "a"
        project_b = tmp_path / "b"
        project_a.mkdir()
        project_b.mkdir()

        (project_a / "source").touch()
        _small_sleep()
        extension_module = project_a / "extension"
        extension_module.touch()
        _small_sleep()
        (project_b / "source").touch()

        project_mtime = _get_project_mtime(project_a, [project_b], extension_module, set())
        assert project_mtime == (project_b / "source").stat().st_mtime

        extension_module.touch()
        project_mtime = _get_project_mtime(project_a, [project_b], extension_module, set())
        assert project_mtime == (project_a / "extension").stat().st_mtime

    def test_extension_module_dir_with_some_newer(self, tmp_path: Path) -> None:
        src_dir = tmp_path / "src"
        extension_dir = tmp_path / "extension_module"
        src_dir.mkdir()
        extension_dir.mkdir()

        (extension_dir / "a").touch()
        _small_sleep()
        (src_dir / "source").touch()
        _small_sleep()
        (extension_dir / "b").touch()

        extension_mtime = _get_installed_package_mtime(extension_dir, set())
        assert extension_mtime == (extension_dir / "a").stat().st_mtime
        project_mtime = _get_project_mtime(tmp_path, [], extension_dir, set())
        assert project_mtime == (src_dir / "source").stat().st_mtime

        _small_sleep()
        (extension_dir / "a").touch()
        extension_mtime = _get_installed_package_mtime(extension_dir, set())
        assert extension_mtime == (extension_dir / "b").stat().st_mtime
        project_mtime = _get_project_mtime(tmp_path, [], extension_dir, set())
        assert project_mtime == (src_dir / "source").stat().st_mtime

    def test_extension_module_dir_with_newer_pycache(self, tmp_path: Path) -> None:
        mixed_src_dir = tmp_path / "mixed_dir"
        mixed_src_dir.mkdir()

        (mixed_src_dir / "__init__.py").touch()
        _small_sleep()
        extension_path = mixed_src_dir / "extension"
        extension_path.touch()  # project is built
        _small_sleep()
        (mixed_src_dir / "__pycache__").mkdir()  # pycache is created later when loaded
        (mixed_src_dir / "__pycache__/some_cache.pyc").touch()

        extension_mtime = _get_installed_package_mtime(extension_path, set())
        assert extension_mtime == extension_path.stat().st_mtime
        project_mtime = _get_project_mtime(tmp_path, [], extension_path, set())
        assert project_mtime == (mixed_src_dir / "__pycache__/some_cache.pyc").stat().st_mtime

        project_mtime = _get_project_mtime(tmp_path, [], extension_path, {"__pycache__"})
        assert project_mtime == extension_path.stat().st_mtime

    def test_extension_outside_project_source(self, tmp_path: Path) -> None:
        project_dir = tmp_path / "project"
        installed_dir = tmp_path / "site-packages"
        project_dir.mkdir()
        installed_dir.mkdir()

        (project_dir / "source").touch()
        _small_sleep()
        extension_path = installed_dir / "extension"
        extension_path.touch()

        project_mtime = _get_project_mtime(project_dir, [], extension_path, set())
        assert project_mtime == (project_dir / "source").stat().st_mtime

        _small_sleep()
        (project_dir / "source").touch()

        project_mtime = _get_project_mtime(project_dir, [], extension_path, set())
        assert project_mtime == (project_dir / "source").stat().st_mtime


def _get_ground_truth_resolved_project_names() -> List[str]:
    # passed in by the test runner
    resolved_packages_path = Path(os.environ["RESOLVED_PACKAGES_PATH"])
    resolved_data = json.loads(resolved_packages_path.read_text())
    return sorted(resolved_data.keys(), key=itemgetter(0))


def _get_ground_truth_resolved_project(project_name: str) -> Dict[str, Any]:
    # passed in by the test runner
    resolved_packages_path = Path(os.environ["RESOLVED_PACKAGES_PATH"])
    resolved_data = json.loads(resolved_packages_path.read_text())
    return resolved_data[project_name]


@pytest.mark.parametrize("project_name", _get_ground_truth_resolved_project_names())
def test_resolve_project(project_name: str) -> None:
    ground_truth = _get_ground_truth_resolved_project(project_name)

    log("ground truth:")
    log(json.dumps(ground_truth, indent=2, sort_keys=True))

    try:
        resolved = _resolve_project(test_crates / project_name)
    except ProjectResolveError:
        calculated = None
    else:
        calculated = {
            "cargo_manifest_path": _optional_path_to_str(resolved.cargo_manifest_path),
            "python_dir": _optional_path_to_str(resolved.python_dir),
            "python_module": _optional_path_to_str(resolved.python_module),
            "extension_module_dir": _optional_path_to_str(resolved.extension_module_dir),
            "module_full_name": resolved.module_full_name,
        }
    log("calculated:")
    log(json.dumps(calculated, indent=2, sort_keys=True))

    assert ground_truth == calculated


def test_build_cache(tmp_path: Path) -> None:
    cache = BuildCache(tmp_path / "build", lock_timeout_seconds=1)

    with cache.lock() as locked_cache:
        dir_1 = locked_cache.tmp_project_dir(tmp_path / "my_module", "my_module")
        dir_2 = locked_cache.tmp_project_dir(tmp_path / "other_place", "my_module")
        assert dir_1 != dir_2

        status1 = BuildStatus(1.2, tmp_path / "source1", ["arg1"], "output1")
        status2 = BuildStatus(1.2, tmp_path / "source2", ["arg2"], "output2")
        locked_cache.store_build_status(status1)
        locked_cache.store_build_status(status2)
        assert locked_cache.get_build_status(tmp_path / "source1") == status1
        assert locked_cache.get_build_status(tmp_path / "source2") == status2
        assert locked_cache.get_build_status(tmp_path / "source3") is None

        status1b = BuildStatus(1.3, tmp_path / "source1", ["arg1b"], "output1b")
        locked_cache.store_build_status(status1b)
        assert locked_cache.get_build_status(tmp_path / "source1") == status1b


def test_uri_to_path() -> None:
    if platform.system() == "Windows":
        assert _uri_to_path("file:///C:/abc/d%20e%20f") == Path(r"C:\abc\d e f")
    else:
        assert _uri_to_path("file:///abc/d%20e%20f") == Path("/abc/d e f")


def test_load_dist_info(tmp_path: Path) -> None:
    dist_info = tmp_path / "package_foo-1.0.0.dist-info"
    dist_info.mkdir(parents=True)
    if platform.system() == "Windows":
        uri = "file:///C:/some%20directory/foo"
        path = Path(r"C:\some directory\foo")
    else:
        uri = "file:///tmp/some%20directory/foo"
        path = Path("/tmp/some directory/foo")

    (dist_info / "direct_url.json").write_text('{"dir_info": {"editable": true}, "url": "' + uri + '"}')

    linked_path, is_editable = _load_dist_info(tmp_path, "package_foo", require_project_target=False)
    assert linked_path == path
    assert is_editable


def _optional_path_to_str(path: Optional[Path]) -> Optional[str]:
    return str(path) if path is not None else None


def _small_sleep() -> None:
    time.sleep(0.05)
