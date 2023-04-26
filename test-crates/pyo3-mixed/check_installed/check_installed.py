#!/usr/bin/env python3
import json
import os.path
import platform
import sys
from pathlib import Path
from subprocess import check_output

from boltons.strutils import slugify

import pyo3_mixed

assert pyo3_mixed.get_42() == 42
assert slugify("First post! Hi!!!!~1    ") == "first_post_hi_1"

script_name = "print_cli_args"
args = ["a", "b", "c"]
[rust_args, python_args] = check_output([script_name, *args], text=True).splitlines()
# The rust vec debug format is also valid json
rust_args = json.loads(rust_args)
python_args = json.loads(python_args)

# On alpine/musl, rust_args is empty so we skip all tests on musl
if len(rust_args) > 0:
    # On linux we get sys.executable, windows resolve the path and mac os gives us a third
    # path (
    # {prefix}/Python.framework/Versions/3.10/Resources/Python.app/Contents/MacOS/Python
    # vs
    # {prefix}/Python.framework/Versions/3.10/bin/python3.10
    # on cirrus ci)
    # On windows, cpython resolves while pypy doesn't.
    # The script for cpython is actually a distinct file from the system interpreter for
    # windows and mac
    if platform.system() == "Linux":
        assert os.path.samefile(rust_args[0], sys.executable), (
            rust_args,
            sys.executable,
            os.path.realpath(rust_args[0]),
            os.path.realpath(sys.executable),
        )

    # Windows can't decide if it's with or without .exe, FreeBSB just doesn't work for some reason
    if platform.system() in ["Darwin", "Linux"]:
        # Unix venv layout (and hopefully also on more exotic platforms)
        print_cli_args = str(Path(sys.prefix).joinpath("bin").joinpath(script_name))
        assert rust_args[1] == print_cli_args, (rust_args, print_cli_args)
        assert python_args[0] == print_cli_args, (python_args, print_cli_args)

    # FreeBSB just doesn't work for some reason
    if platform.system() in ["Darwin", "Linux", "Windows"]:
        # Rust contains the python executable as first argument but python does not
        assert rust_args[2:] == args, rust_args
        assert python_args[1:] == args, python_args

print("SUCCESS")
