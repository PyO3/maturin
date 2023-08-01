#!/usr/bin/env python3

import pyo3_mixed_implicit.some_rust as some_rust


def test_get_rust_and_python():
    assert some_rust.get_22() == 22
    assert some_rust.double(lambda: 4) == 8


print("SUCCESS")
