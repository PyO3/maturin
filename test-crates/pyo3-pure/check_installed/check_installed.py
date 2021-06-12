#!/usr/bin/env python3
import os

import pyo3_pure

assert pyo3_pure.DummyClass.get_42() == 42

# Check type stub
install_path = os.path.join(os.path.dirname(pyo3_pure.__file__))
assert os.path.exists(os.path.join(install_path, "__init__.pyi"))
assert os.path.exists(os.path.join(install_path, "py.typed"))

print("SUCCESS")
