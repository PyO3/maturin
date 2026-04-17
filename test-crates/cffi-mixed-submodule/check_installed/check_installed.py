#!/usr/bin/env python3

from cffi_mixed_submodule.rust_module import rust

point = rust.lib.get_origin()
assert rust.lib.is_in_range(point, 0.0)

print("SUCCESS")
