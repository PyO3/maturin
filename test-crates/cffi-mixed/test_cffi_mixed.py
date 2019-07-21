#!/usr/bin/env python3

import cffi_mixed

from cffi_mixed import Line


def test_in_range():
    point = cffi_mixed.lib.get_origin()
    point.x = 10
    point.y = 10

    assert not cffi_mixed.lib.is_in_range(point, 14)
    assert cffi_mixed.lib.is_in_range(point, 15)


def test_ffi():
    point = cffi_mixed.ffi.new("Point *", (10, 20))
    assert point.x == 10
    assert point.y == 20


def test_line():
    line = Line(2, 5, 6, 8)
    assert line.length() == 5
    assert str(line) == "Line from (2.0,5.0) to (6.0,8.0)"
