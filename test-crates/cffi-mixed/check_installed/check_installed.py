#!/usr/bin/env python3

import cffi_mixed

from cffi_mixed import Line

point = cffi_mixed.lib.get_origin()
point.x = 10
point.y = 10
assert cffi_mixed.lib.is_in_range(point, 15)

assert Line(2, 5, 6, 8).length() == 5

print("SUCCESS")
