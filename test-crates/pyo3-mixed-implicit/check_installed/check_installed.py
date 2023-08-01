#!/usr/bin/env python3

import pyo3_mixed_implicit.some_rust as some_rust

assert some_rust.get_22() == 22
assert some_rust.double(lambda: 4) == 16

print("SUCCESS")
