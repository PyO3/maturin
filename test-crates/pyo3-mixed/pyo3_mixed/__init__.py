from .pyo3_mixed import get_21, print_cli_args  # noqa: F401
from .python_module.double import double


def get_42() -> int:
    return double(get_21)
