from .pyo3_mixed_workspace_py import get_21
from .python_module.double import double


def get_42() -> int:
    return double(get_21)
