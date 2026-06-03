#!/usr/bin/env python3
import os

import pyo3_stub_generation_mixed

assert pyo3_stub_generation_mixed.get_42() == 42

# Check type stub
install_path = os.path.join(os.path.dirname(pyo3_stub_generation_mixed.__file__))
assert os.path.exists(os.path.join(install_path, "__init__.py"))
assert os.path.exists(os.path.join(install_path, "pyo3_stub_generation_mixed/__init__.pyi"))
assert os.path.exists(os.path.join(install_path, "pyo3_stub_generation_mixed/submodule.pyi"))

print("SUCCESS")
