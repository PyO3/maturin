#!/usr/bin/env python3

import uniffi_export_and_udl

struct = uniffi_export_and_udl.NumbersToAdd(numbers=[1, 2, 3])
assert uniffi_export_and_udl.add(struct.numbers[0], struct.numbers[1]) == 3

print("SUCCESS")
