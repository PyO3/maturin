from .python_module.double import double
from .rust_module.rust import get_21


def get_42() -> int:
    return double(get_21)
