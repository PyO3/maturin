# ruff: noqa: E402
import logging

logging.basicConfig(format="%(name)s [%(levelname)s] %(message)s", level=logging.DEBUG)

from maturin import import_hook

import_hook.reset_logger()
# increase default timeout as under heavy load on a weak machine
# the workers may be waiting on the locks for a long time.
import_hook.install(lock_timeout_seconds=10 * 60)

import packages.my_rust_module

assert packages.my_rust_module.do_something(1, 2) == 3

print("SUCCESS")
