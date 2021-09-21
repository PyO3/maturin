#!/usr/bin/env python3
import os
import subprocess

import pyo3_pure

assert pyo3_pure.DummyClass.get_42() == 42

# Check type stub
install_path = os.path.join(os.path.dirname(pyo3_pure.__file__))
assert os.path.exists(os.path.join(install_path, "__init__.pyi"))
assert os.path.exists(os.path.join(install_path, "py.typed"))

# Check entrypoints
assert subprocess.run(["get_42"]).returncode == 42

print("SUCCESS")
