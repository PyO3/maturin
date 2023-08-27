# ruff: noqa: E402
import logging

logging.basicConfig(format="%(name)s [%(levelname)s] %(message)s", level=logging.DEBUG)

from maturin import import_hook

import_hook.reset_logger()
import_hook.install()

from my_script import get_num

print(f"get_num = {get_num()}")

try:
    from my_script import get_other_num
except ImportError:
    print("failed to import get_other_num")
else:
    print(f"get_other_num = {get_other_num()}")

print("SUCCESS")
