#!/usr/bin/env python3

import cffi_mixed


def test_range():
    point = cffi_mixed.lib.get_origin()
    point.x = 10
    point.y = 10

    assert not cffi_mixed.lib.is_in_range(point, 14)
    assert cffi_mixed.lib.is_in_range(point, 15)


def test_ffi():
    assert cffi_mixed.ffi is not None
