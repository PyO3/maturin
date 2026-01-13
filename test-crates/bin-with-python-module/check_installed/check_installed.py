"""
Test that both the binary and Python module work correctly after editable install.
This tests the fix for https://github.com/PyO3/maturin/issues/2933
"""

import subprocess


def main():
    # Test 1: Check that the Python module works
    from bin_with_python_module import get_version

    version = get_version()
    if version != "0.1.0":
        raise Exception(f"Expected version '0.1.0', got '{version}'")

    # Test 2: Check that the binary is installed and works
    # The binary should be in PATH after editable install
    result = subprocess.run(
        ["bin-with-python-module"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise Exception(
            f"Binary failed with code {result.returncode}: stdout={result.stdout!r}, stderr={result.stderr!r}"
        )

    expected_output = "bin-with-python-module 0.1.0"
    if expected_output not in result.stdout:
        raise Exception(f"Expected output containing '{expected_output}', got '{result.stdout}'")

    print("SUCCESS")


if __name__ == "__main__":
    main()
