#!/usr/bin/env python3

import pyo3_mixed_with_path_dep


def test_get_42():
    assert pyo3_mixed_with_path_dep.get_42() == 42
