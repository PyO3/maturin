import platform
from pathlib import Path
import sysconfig

import pyo3_mixed


class MaturinTestError(Exception):
    pass


def main():
    # e.g. `...\test-crates\pyo3-mixed-msvc-debuginfo\python\pyo3_mixed\__init__.py`
    init_py_path = Path(pyo3_mixed.__file__)

    ext_suffix = sysconfig.get_config_var("EXT_SUFFIX")

    # set by `module-name = "pyo3_mixed._pyo3_lib"` in `pyproject.toml`
    # e.g. `_pyo3_lib.cp310-win_amd64.pyd`
    lib_pyd_path = init_py_path.with_name(f"_pyo3_lib{ext_suffix}")
    if not lib_pyd_path.exists():
        raise MaturinTestError(f"{lib_pyd_path} does not exist")

    # set by `lib.name = "pyo3_mixed"` in `Cargo.toml`
    lib_debuginfo_path = init_py_path.with_name("pyo3_mixed.pdb")
    if not lib_debuginfo_path.exists():
        raise MaturinTestError(f"{lib_debuginfo_path} does not exist")


if __name__ == "__main__":
    if not platform.system() == "Windows":
        raise MaturinTestError("This test is only supported on MSVC")

    main()

    print("SUCCESS")
