from __future__ import annotations

import os
import sys
from pathlib import Path
import sysconfig


def get_maturin_path() -> Path | None:
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

    os.execv(maturin, [str(maturin)] + sys.argv[1:])
