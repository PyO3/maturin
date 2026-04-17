#!/usr/bin/env python3
import cffi_mixed_with_path_dep

assert cffi_mixed_with_path_dep.lib.get_21() == 21, "get_21 did not return 21"
assert cffi_mixed_with_path_dep.lib.add_21(21) == 42, "add_21 did not return 42"
assert cffi_mixed_with_path_dep.lib.is_half(21, 42), "21 is not half of 42"
assert not cffi_mixed_with_path_dep.lib.is_half(21, 73), "21 is half of 73"

print("SUCCESS")
