# ruff: noqa: E402
import logging
import sys
from pathlib import Path

logging.basicConfig(format="%(name)s [%(levelname)s] %(message)s", level=logging.DEBUG)

from maturin import import_hook
from maturin.import_hook.settings import MaturinSettings, MaturinSettingsProvider

import_hook.reset_logger()


class CustomSettingsProvider(MaturinSettingsProvider):
    def get_settings(self, module_path: str, source_path: Path) -> MaturinSettings:
        if len(sys.argv) > 1 and sys.argv[1] == "LARGE_NUMBER":
            print(f"building {module_path} with large_number feature enabled")
            return MaturinSettings(features=["pyo3/extension-module", "large_number"])
        else:
            print(f"building {module_path} with default settings")
            return MaturinSettings.default()


import_hook.install(settings=CustomSettingsProvider())


from my_script import get_num

print(f"get_num = {get_num()}")
print("SUCCESS")
