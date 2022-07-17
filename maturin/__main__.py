import os
import sys
from pathlib import Path
import sysconfig

if __name__ == "__main__":
    scripts_dir = sysconfig.get_path("scripts")
    maturin = Path(scripts_dir) / "maturin"
    os.execv(maturin, [str(maturin)] + sys.argv[1:])
