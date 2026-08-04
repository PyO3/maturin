#!/usr/bin/env python3

import a

assert a.random_enum(a.RandomEnum.A) == 0
assert a.random_enum(a.RandomEnum.B) == 1

print("SUCCESS")
