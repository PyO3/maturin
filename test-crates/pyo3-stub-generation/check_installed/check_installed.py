#!/usr/bin/env python3
import os

import pyo3_stub_generation

assert pyo3_stub_generation.func(42) == 42

# Check type stub
install_path = os.path.join(os.path.dirname(pyo3_stub_generation.__file__))
assert os.path.exists(os.path.join(install_path, "__init__.pyi"))
assert os.path.exists(os.path.join(install_path, "submodule.pyi"))
assert os.path.exists(os.path.join(install_path, "py.typed"))

print("SUCCESS")
