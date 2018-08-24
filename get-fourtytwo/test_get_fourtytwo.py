import get_fourtytwo

import pytest


def test_static():
    assert get_fourtytwo.fourtytwo == 42


def test_class():
    assert get_fourtytwo.DummyClass.get_42() == 42


@pytest.mark.xfail(raises=AssertionError)
def test_function():
    assert get_fourtytwo.DummyClass == 42
