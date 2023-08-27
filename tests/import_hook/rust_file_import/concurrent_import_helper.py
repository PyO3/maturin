# ruff: noqa: E402
import logging

logging.basicConfig(format="%(name)s [%(levelname)s] %(message)s", level=logging.DEBUG)

from maturin import import_hook

import_hook.reset_logger()
import_hook.install()

import packages.my_rust_module

assert packages.my_rust_module.do_something(1, 2) == 3

print("SUCCESS")
