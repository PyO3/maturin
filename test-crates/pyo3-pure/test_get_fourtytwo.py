#!/usr/bin/env python3

import pyo3_pure

import pytest


def test_static():
    assert pyo3_pure.fourtytwo == 42


def test_class():
    assert pyo3_pure.DummyClass.get_42() == 42


def test_function():
    with pytest.raises(AssertionError):
        assert pyo3_pure.DummyClass == 42
