import uniffi_export_and_udl


def test_add():
    assert uniffi_export_and_udl.add(1, 2) == 3
