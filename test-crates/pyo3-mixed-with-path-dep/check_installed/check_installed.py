#!/usr/bin/env python3
import pyo3_mixed_with_path_dep

assert pyo3_mixed_with_path_dep.get_42() == 42, "get_42 did not return 42"

assert pyo3_mixed_with_path_dep.is_half(21, 42), "21 is not half of 42"
assert not pyo3_mixed_with_path_dep.is_half(21, 73), "21 is half of 63"

print("SUCCESS")
