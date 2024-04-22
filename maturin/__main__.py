from __future__ import annotations

import os
import sys
from pathlib import Path
import sysconfig
from typing import Optional


def get_maturin_path() -> Optional[Path]:
    SCRIPT_NAME = "maturin"

    def script_dir(scheme: str) -> str:
        return sysconfig.get_path("scripts", scheme)

    def script_exists(dir: str) -> bool:
        for _, _, files in os.walk(dir):
            for f in files:
                name, *_ = os.path.splitext(f)
                if name == SCRIPT_NAME:
                    return True

        return False

    paths = list(
        filter(
            script_exists,
            filter(os.path.exists, map(script_dir, sysconfig.get_scheme_names())),
        )
    )

    if paths:
        return Path(paths[0]) / SCRIPT_NAME

    return None


if __name__ == "__main__":
    maturin = get_maturin_path()
    if maturin is None:
        print("Unable to find `maturin` script")
        exit(1)

    if sys.platform == "win32":
        import subprocess

        code = subprocess.call([str(maturin)] + sys.argv[1:])
        sys.exit(code)
    else:
        os.execv(maturin, [str(maturin)] + sys.argv[1:])
