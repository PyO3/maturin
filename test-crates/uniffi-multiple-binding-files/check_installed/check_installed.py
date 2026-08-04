#!/usr/bin/env python3

import uniffi_multiple_binding_files

assert uniffi_multiple_binding_files.get_status() == uniffi_multiple_binding_files.mylib.Status.COMPLETE

print("SUCCESS")
