import sys
import platform
from pathlib import Path


class MaturinTestError(Exception):
    pass


def main():
    # e.g. `venv\Scripts`
    scripts_dir = Path(sys.executable).parent

    # set by `bin.name = "pyo3-bin"` in `Cargo.toml`
    exe_binding_path = scripts_dir / "pyo3-bin.exe"
    if not exe_binding_path.exists():
        raise MaturinTestError(f"{exe_binding_path} does not exist")

    # the pdb file of `foo-bar.exe` is `foo_bar.pdb`
    exe_binding_debuginfo_path = scripts_dir / "pyo3_bin.pdb"
    if not exe_binding_debuginfo_path.exists():
        raise MaturinTestError(f"{exe_binding_debuginfo_path} does not exist")


if __name__ == "__main__":
    if not platform.system() == "Windows":
        raise MaturinTestError("This test is only supported on MSVC")

    main()

    print("SUCCESS")
