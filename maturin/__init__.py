#!/usr/bin/env python3
"""
maturin's implementation of the PEP 517 interface. Calls maturin through subprocess

Currently, the "return value" of the rust implementation is the last line of stdout

On windows, apparently pip's subprocess handling sets stdout to some windows encoding (e.g. cp1252 on my machine),
even though the terminal supports utf8. Writing directly to the binary stdout buffer avoids encoding errors due to
maturin's emojis.
"""

import os
import shutil
import subprocess
import sys
from subprocess import SubprocessError
from typing import Dict

import toml


def get_config() -> Dict[str, str]:
    with open("pyproject.toml", encoding="utf-8") as fp:
        pyproject_toml = toml.load(fp)
    return pyproject_toml.get("tool", {}).get("maturin", {})


# noinspection PyUnusedLocal
def _build_wheel(
    wheel_directory, config_settings=None, metadata_directory=None, editable=False
):
    # PEP 517 specifies that only `sys.executable` points to the correct
    # python interpreter
    command = ["maturin", "pep517", "build-wheel", "-i", sys.executable]
    if editable:
        command.append("--editable")

    print("Running `{}`".format(" ".join(command)))
    sys.stdout.flush()
    result = subprocess.run(command, stdout=subprocess.PIPE)
    sys.stdout.buffer.write(result.stdout)
    sys.stdout.flush()
    if result.returncode != 0:
        sys.stderr.write(
            f"Error: command {command} returned non-zero exit status {result.returncode}\n"
        )
        sys.exit(1)
    output = result.stdout.decode(errors="replace")
    wheel_path = output.strip().splitlines()[-1]
    filename = os.path.basename(wheel_path)
    shutil.copy2(wheel_path, os.path.join(wheel_directory, filename))
    return filename


# noinspection PyUnusedLocal
def build_wheel(wheel_directory, config_settings=None, metadata_directory=None):
    return _build_wheel(wheel_directory, config_settings, metadata_directory)


# noinspection PyUnusedLocal
def build_sdist(sdist_directory, config_settings=None):
    command = ["maturin", "pep517", "write-sdist", "--sdist-directory", sdist_directory]

    print("Running `{}`".format(" ".join(command)))
    sys.stdout.flush()
    result = subprocess.run(command, stdout=subprocess.PIPE)
    sys.stdout.buffer.write(result.stdout)
    sys.stdout.flush()
    if result.returncode != 0:
        sys.stderr.write(
            f"Error: command {command} returned non-zero exit status {result.returncode}\n"
        )
        sys.exit(1)
    output = result.stdout.decode(errors="replace")
    return output.strip().splitlines()[-1]


# noinspection PyUnusedLocal
def get_requires_for_build_wheel(config_settings=None):
    if get_config().get("bindings") == "cffi":
        return ["cffi"]
    else:
        return []


# noinspection PyUnusedLocal
def build_editable(wheel_directory, config_settings=None, metadata_directory=None):
    return _build_wheel(
        wheel_directory, config_settings, metadata_directory, editable=True
    )


# Requirements to build an editable are the same as for a wheel
get_requires_for_build_editable = get_requires_for_build_wheel


# noinspection PyUnusedLocal
def get_requires_for_build_sdist(config_settings=None):
    return []


# noinspection PyUnusedLocal
def prepare_metadata_for_build_wheel(metadata_directory, config_settings=None):
    print("Checking for Rust toolchain....")
    is_cargo_installed = False
    try:
        output = subprocess.check_output(["cargo", "--version"]).decode(
            "utf-8", "ignore"
        )
        if "cargo" in output:
            is_cargo_installed = True
    except (FileNotFoundError, SubprocessError):
        pass

    if not is_cargo_installed:
        sys.stderr.write(
            "\nCargo, the Rust package manager, is not installed or is not on PATH.\n"
            "This package requires Rust and Cargo to compile extensions. Install it through\n"
            "the system's package manager or via https://rustup.rs/\n\n"
        )
        sys.exit(1)

    command = [
        "maturin",
        "pep517",
        "write-dist-info",
        "--metadata-directory",
        metadata_directory,
        # PEP 517 specifies that only `sys.executable` points to the correct
        # python interpreter
        "--interpreter",
        sys.executable,
    ]

    print("Running `{}`".format(" ".join(command)))
    try:
        output = subprocess.check_output(command)
    except subprocess.CalledProcessError as e:
        sys.stderr.write(f"Error running maturin: {e}\n")
        sys.exit(1)
    sys.stdout.buffer.write(output)
    sys.stdout.flush()
    output = output.decode(errors="replace")
    return output.strip().splitlines()[-1]


# Metadata for editable are the same as for a wheel
prepare_metadata_for_build_editable = prepare_metadata_for_build_wheel
