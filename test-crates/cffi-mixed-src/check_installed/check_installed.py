#!/usr/bin/env python3

import cffi_mixed_src as cffi_mixed

point = cffi_mixed.lib.get_origin()
assert cffi_mixed.lib.is_in_range(point, 0.0)

print("SUCCESS")
