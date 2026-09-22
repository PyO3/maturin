import sys
from pathlib import Path

project_root = Path(__file__).resolve().parent
sys.path = [
    path
    for path in sys.path
    if path and Path(path).resolve() != project_root
]

import attrs
import pyo3_mixed


assert attrs is not None
assert pyo3_mixed.get_42() == 42
pyo3_mixed.print_cli_args()
