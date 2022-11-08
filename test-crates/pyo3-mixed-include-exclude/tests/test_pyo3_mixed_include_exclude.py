#!/usr/bin/env python3

import pyo3_mixed_include_exclude


def test_get_42():
    assert pyo3_mixed_include_exclude.get_42() == 42
