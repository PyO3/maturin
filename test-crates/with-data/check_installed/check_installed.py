import locale
import sys
from pathlib import Path

import with_data

assert with_data.lib.one() == 1
assert with_data.ffi.string(with_data.lib.say_hello()).decode() == "hello"

venv_root = Path(sys.prefix)

installed_data = (
    venv_root.joinpath("data_subdir")
    .joinpath("hello.txt")
    # With the default encoding, python under windows fails to read the file correctly :/
    .read_text(encoding="utf-8")
    .strip()
)
assert installed_data == "Hi! 😊", (
    installed_data,
    "Hi! 😊",
    locale.getpreferredencoding(),
)
header_file = (
    venv_root.joinpath("include")
    .joinpath("site")
    .joinpath(f"python{sys.version_info.major}.{sys.version_info.minor}")
    .joinpath("with-data")
    .joinpath("empty.h")
)
assert header_file.is_file(), header_file

print("SUCCESS")
