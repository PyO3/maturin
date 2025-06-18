"""Support installing rust before compiling (bootstrapping) maturin.

Installing a package that uses maturin as build backend on a platform without maturin
binaries, we install rust in a cache directory if the user doesn't have a rust
installation already. Since this bootstrapping requires more dependencies but is only
required if rust is missing, we check if cargo is present before requesting those
dependencies.

https://setuptools.pypa.io/en/stable/build_meta.html#dynamic-build-dependencies-and-other-build-meta-tweaks
"""

from __future__ import annotations

import os
import shutil
from typing import Any

# noinspection PyUnresolvedReferences
from setuptools.build_meta import *  # noqa:F403


def get_requires_for_build_wheel(config_settings: dict[str, Any] | None = None) -> list[str]:
    if not os.environ.get("MATURIN_NO_INSTALL_RUST") and not shutil.which("cargo"):
        return ["puccinialin>=0.1,<0.2"]
    return []


def get_requires_for_build_sdist(config_settings: dict[str, Any] | None = None) -> list[str]:
    if not os.environ.get("MATURIN_NO_INSTALL_RUST") and not shutil.which("cargo"):
        return ["puccinialin>=0.1,<0.2"]
    return []
