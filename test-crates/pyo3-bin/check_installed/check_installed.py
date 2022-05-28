import os
import platform
import sys
from subprocess import check_output


def main():
    if platform.system().lower() == "windows":
        # Add sys.base_prefix which contains python3y.dll to PATH
        # otherwise running `pyo3-bin` might return exit code 3221225781
        path = os.environ["PATH"]
        path = path + os.pathsep + sys.base_prefix
        os.environ["PATH"] = path

    output = check_output(["pyo3-bin"]).decode("utf-8").strip()
    if not output == "Hello, world!":
        raise Exception(output)
    print("SUCCESS")


if __name__ == "__main__":
    main()
