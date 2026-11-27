#!/usr/bin/env python3

import _pyo3_mixed_top_level
import pyo3_mixed_top_level

assert _pyo3_mixed_top_level.get_21() == 21
assert pyo3_mixed_top_level.get_42() == 42

print("SUCCESS")
