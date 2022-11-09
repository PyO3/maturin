from .python_module.double import double
from .pyo3_mixed_include_exclude import get_21


def get_42() -> int:
    return double(get_21)
