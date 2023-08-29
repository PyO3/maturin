# ruff: noqa: E402
import logging
import sys

logging.basicConfig(format="%(name)s [%(levelname)s] %(message)s", level=logging.DEBUG)

from maturin import import_hook
from maturin.import_hook.settings import MaturinSettings

import_hook.reset_logger()

if len(sys.argv) > 1 and sys.argv[1] == "LARGE_NUMBER":
    print("building with large_number feature enabled")
    settings = MaturinSettings(features=["pyo3/extension-module", "large_number"])
else:
    print("building with default settings")
    settings = MaturinSettings.default()

import_hook.install(settings=settings)


from my_script import get_num

print(f"get_num = {get_num()}")
print("SUCCESS")
