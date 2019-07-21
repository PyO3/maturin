#!/usr/bin/env python3

import cffi_pure

point = cffi_pure.lib.get_origin()
point.x = 10
point.y = 10
assert cffi_pure.lib.is_in_range(point, 15)

print("SUCCESS")
