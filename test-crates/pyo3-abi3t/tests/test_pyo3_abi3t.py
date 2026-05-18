#!/usr/bin/env python3

import pyo3_abi3t

import pytest


def test_static():
    assert pyo3_abi3t.fourtytwo == 42


def test_class():
    assert pyo3_abi3t.DummyClass.get_42() == 42


def test_function():
    with pytest.raises(AssertionError):
        assert pyo3_abi3t.DummyClass == 42
