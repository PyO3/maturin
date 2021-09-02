#!/usr/bin/env python3

import pyo3_mixed_submodule


def test_get_42():
    assert pyo3_mixed_submodule.get_42() == 42
