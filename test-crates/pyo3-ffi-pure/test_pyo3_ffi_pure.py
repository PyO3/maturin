#!/usr/bin/env python3

import pyo3_ffi_pure


def test_static():
    assert pyo3_ffi_pure.sum(2, 40) == 42
