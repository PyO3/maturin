#!/usr/bin/env python3

import cffi_mixed

point1 = cffi_mixed.lib.make_point(10, 10)
point2 = cffi_mixed.lib.make_point(2, 2)
sum = cffi_mixed.add_points(point1, point2)

assert sum.x == 12 and sum.y == 12

print("SUCCESS")
