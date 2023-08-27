# ruff: noqa: E402
import logging

logging.basicConfig(format="%(name)s [%(levelname)s] %(message)s", level=logging.DEBUG)

from maturin import import_hook

import_hook.reset_logger()
import_hook.install()

from .packages import my_py_module

assert my_py_module.do_something_py(1, 2) == 3

from .packages import my_rust_module

assert my_rust_module.do_something(1, 2) == 3

from .packages import my_rust_module

assert my_rust_module.do_something(1, 2) == 3


# modules with the same name do not clash
from .packages.subpackage import my_rust_module as other_module

assert other_module.get_num() == 42

print("SUCCESS")
