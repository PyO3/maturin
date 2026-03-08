#!/usr/bin/env python3

import cffi_mixed_implicit.some_rust as some_rust

point = some_rust.lib.get_origin()
assert some_rust.lib.is_in_range(point, 0.0)

print("SUCCESS")
