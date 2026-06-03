#!/usr/bin/env python3
import os

import pyo3_stub_generation_mixed_py_subdir

assert pyo3_stub_generation_mixed_py_subdir.get_42() == 42

# Check type stub
install_path = os.path.join(os.path.dirname(pyo3_stub_generation_mixed_py_subdir.__file__))
assert os.path.exists(os.path.join(install_path, "__init__.py"))
assert os.path.exists(os.path.join(install_path, "_pyo3_mixed.pyi"))

print("SUCCESS")
