#!/usr/bin/env python3

import points


def test_range():
    point = points.lib.get_origin()
    point.x = 10
    point.y = 10

    assert not points.lib.is_in_range(point, 14)
    assert points.lib.is_in_range(point, 15)


def test_ffi():
    assert points.ffi is not None
