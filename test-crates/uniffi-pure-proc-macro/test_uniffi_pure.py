import uniffi_pure_proc_macro as uniffi_pure


def test_add():
    assert uniffi_pure.add(1, 2) == 3
