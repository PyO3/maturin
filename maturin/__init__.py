#!/usr/bin/env python3
"""
maturin's implementation of the PEP 517 interface. Calls maturin through subprocess

Currently, the "return value" of the rust implementation is the last line of stdout

On windows, apparently pip's subprocess handling sets stdout to some windows encoding (e.g. cp1252 on my machine),
even though the terminal supports utf8. Writing directly to the binary stdout buffer avoids encoding errors due to
maturin's emojis.
"""
from __future__ import annotations

import os
import platform
import shlex
import shutil
import struct
import subprocess
import sys
from subprocess import SubprocessError
from typing import Any, Dict, Mapping

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib  # type: ignore


def get_config() -> Dict[str, str]:
    with open("pyproject.toml", "rb") as fp:
        pyproject_toml = tomllib.load(fp)
    return pyproject_toml.get("tool", {}).get("maturin", {})


def get_maturin_pep517_args() -> list[str]:
    args = shlex.split(os.getenv("MATURIN_PEP517_ARGS", ""))
    return args


def _additional_pep517_args() -> list[str]:
    # Support building for 32-bit Python on x64 Windows
    if platform.system().lower() == "windows" and platform.machine().lower() == "amd64":
        pointer_width = struct.calcsize("P") * 8
        if pointer_width == 32:
            return ["--target", "i686-pc-windows-msvc"]
    return []


# noinspection PyUnusedLocal
def _build_wheel(
    wheel_directory: str,
    config_settings: Mapping[str, Any] | None = None,
    metadata_directory: str | None = None,
    editable: bool = False,
) -> str:
    # PEP 517 specifies that only `sys.executable` points to the correct
    # python interpreter
    command = [
        "maturin",
        "pep517",
        "build-wheel",
        "-i",
        sys.executable,
        "--compatibility",
        "off",
    ]
    command.extend(_additional_pep517_args())
    if editable:
        command.append("--editable")

    pep517_args = get_maturin_pep517_args()
    if pep517_args:
        command.extend(pep517_args)

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
def build_wheel(
    wheel_directory: str,
    config_settings: Mapping[str, Any] | None = None,
    metadata_directory: str | None = None,
) -> str:
    return _build_wheel(wheel_directory, config_settings, metadata_directory)


# noinspection PyUnusedLocal
def build_sdist(
    sdist_directory: str, config_settings: Mapping[str, Any] | None = None
) -> str:
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
def get_requires_for_build_wheel(
    config_settings: Mapping[str, Any] | None = None
) -> list[str]:
    if get_config().get("bindings") == "cffi":
        return ["cffi"]
    else:
        return []


# noinspection PyUnusedLocal
def build_editable(
    wheel_directory: str,
    config_settings: Mapping[str, Any] | None = None,
    metadata_directory: str | None = None,
) -> str:
    return _build_wheel(
        wheel_directory, config_settings, metadata_directory, editable=True
    )


# Requirements to build an editable are the same as for a wheel
get_requires_for_build_editable = get_requires_for_build_wheel


# noinspection PyUnusedLocal
def get_requires_for_build_sdist(
    config_settings: Mapping[str, Any] | None = None
) -> list:
    return []


# noinspection PyUnusedLocal
def prepare_metadata_for_build_wheel(
    metadata_directory: str, config_settings: Mapping[str, Any] | None = None
) -> str:
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
    command.extend(_additional_pep517_args())
    pep517_args = get_maturin_pep517_args()
    if pep517_args:
        command.extend(pep517_args)

    print("Running `{}`".format(" ".join(command)))
    try:
        _output = subprocess.check_output(command)
    except subprocess.CalledProcessError as e:
        sys.stderr.write(f"Error running maturin: {e}\n")
        sys.exit(1)
    sys.stdout.buffer.write(_output)
    sys.stdout.flush()
    output = _output.decode(errors="replace")
    return output.strip().splitlines()[-1]


# Metadata for editable are the same as for a wheel
prepare_metadata_for_build_editable = prepare_metadata_for_build_wheel
