from .pyo3_mixed_with_path_dep import get_21, add_21, is_half

__all__ = ["get_21", "add_21", "is_half", "get_42"]


def get_42() -> int:
    return add_21(get_21())
