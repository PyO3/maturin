from .subpackage import my_rust_module


def foo() -> int:
    return my_rust_module.get_num() + 100
