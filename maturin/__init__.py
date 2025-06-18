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
from typing import Any, Dict, Mapping, List, Optional


def get_config() -> Dict[str, str]:
    try:
        import tomllib
    except ModuleNotFoundError:
        import tomli as tomllib  # type: ignore

    with open("pyproject.toml", "rb") as fp:
        pyproject_toml = tomllib.load(fp)
    return pyproject_toml.get("tool", {}).get("maturin", {})


def get_maturin_pep517_args(config_settings: Optional[Mapping[str, Any]] = None) -> List[str]:
    build_args = None
    if config_settings:
        # TODO: Deprecate and remove build-args in favor of maturin.build-args in maturin 2.0
        build_args = config_settings.get("maturin.build-args", config_settings.get("build-args"))
    if build_args is None:
        env_args = os.getenv("MATURIN_PEP517_ARGS", "")
        args = shlex.split(env_args)
    elif isinstance(build_args, str):
        args = shlex.split(build_args)
    else:
        args = build_args
    return args


def _get_sys_executable() -> str:
    executable = sys.executable
    if os.getenv("MATURIN_PEP517_USE_BASE_PYTHON") in {"1", "true"}:
        # Use the base interpreter path when running inside a venv to avoid recompilation
        # when switching between venvs
        base_executable = getattr(sys, "_base_executable")
        if base_executable and os.path.exists(base_executable):
            executable = os.path.realpath(base_executable)
    return executable


def _additional_pep517_args() -> List[str]:
    # Support building for 32-bit Python on x64 Windows
    if platform.system().lower() == "windows" and platform.machine().lower() == "amd64":
        pointer_width = struct.calcsize("P") * 8
        if pointer_width == 32:
            return ["--target", "i686-pc-windows-msvc"]
    return []


def _get_env() -> Optional[Dict[str, str]]:
    if not os.environ.get("MATURIN_NO_INSTALL_RUST") and not shutil.which("cargo"):
        from puccinialin import setup_rust

        print("Rust not found, installing into a temporary directory")
        extra_env = setup_rust()
        return {**os.environ, **extra_env}
    else:
        return None


# noinspection PyUnusedLocal
def _build_wheel(
    wheel_directory: str,
    config_settings: Optional[Mapping[str, Any]] = None,
    metadata_directory: Optional[str] = None,
    editable: bool = False,
) -> str:
    # PEP 517 specifies that only `sys.executable` points to the correct
    # python interpreter
    base_command = [
        "maturin",
        "pep517",
        "build-wheel",
        "-i",
        _get_sys_executable(),
    ]
    options = _additional_pep517_args()
    if editable:
        options.append("--editable")

    pep517_args = get_maturin_pep517_args(config_settings)
    if pep517_args:
        options.extend(pep517_args)

    if "--compatibility" not in options and "--manylinux" not in options:
        # default to off if not otherwise specified
        options = ["--compatibility", "off", *options]

    command = [*base_command, *options]

    print("Running `{}`".format(" ".join(command)))
    sys.stdout.flush()
    result = subprocess.run(command, stdout=subprocess.PIPE, env=_get_env())
    sys.stdout.buffer.write(result.stdout)
    sys.stdout.flush()
    if result.returncode != 0:
        sys.stderr.write(f"Error: command {command} returned non-zero exit status {result.returncode}\n")
        sys.exit(1)
    output = result.stdout.decode(errors="replace")
    wheel_path = output.strip().splitlines()[-1]
    filename = os.path.basename(wheel_path)
    shutil.copy2(wheel_path, os.path.join(wheel_directory, filename))
    return filename


# noinspection PyUnusedLocal
def build_wheel(
    wheel_directory: str,
    config_settings: Optional[Mapping[str, Any]] = None,
    metadata_directory: Optional[str] = None,
) -> str:
    return _build_wheel(wheel_directory, config_settings, metadata_directory)


# noinspection PyUnusedLocal
def build_sdist(sdist_directory: str, config_settings: Optional[Mapping[str, Any]] = None) -> str:
    command = ["maturin", "pep517", "write-sdist", "--sdist-directory", sdist_directory]

    print("Running `{}`".format(" ".join(command)))
    sys.stdout.flush()
    result = subprocess.run(command, stdout=subprocess.PIPE, env=_get_env())
    sys.stdout.buffer.write(result.stdout)
    sys.stdout.flush()
    if result.returncode != 0:
        sys.stderr.write(f"Error: command {command} returned non-zero exit status {result.returncode}\n")
        sys.exit(1)
    output = result.stdout.decode(errors="replace")
    return output.strip().splitlines()[-1]


# noinspection PyUnusedLocal
def get_requires_for_build_wheel(config_settings: Optional[Mapping[str, Any]] = None) -> List[str]:
    if get_config().get("bindings") == "cffi":
        requirements = ["cffi"]
    else:
        requirements = []
    if not os.environ.get("MATURIN_NO_INSTALL_RUST") and not shutil.which("cargo"):
        requirements += ["puccinialin"]
    return requirements


# noinspection PyUnusedLocal
def build_editable(
    wheel_directory: str,
    config_settings: Optional[Mapping[str, Any]] = None,
    metadata_directory: Optional[str] = None,
) -> str:
    return _build_wheel(wheel_directory, config_settings, metadata_directory, editable=True)


# Requirements to build an editable are the same as for a wheel
get_requires_for_build_editable = get_requires_for_build_wheel


# noinspection PyUnusedLocal
def get_requires_for_build_sdist(config_settings: Optional[Mapping[str, Any]] = None) -> List[str]:
    requirements = []
    if not os.environ.get("MATURIN_NO_INSTALL_RUST") and not shutil.which("cargo"):
        requirements += ["puccinialin"]
    return requirements


# noinspection PyUnusedLocal
def prepare_metadata_for_build_wheel(
    metadata_directory: str, config_settings: Optional[Mapping[str, Any]] = None
) -> str:
    print("Checking for Rust toolchain....")
    is_cargo_installed = False
    try:
        output = subprocess.check_output(["cargo", "--version"], env=_get_env()).decode("utf-8", "ignore")
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
        _get_sys_executable(),
    ]
    command.extend(_additional_pep517_args())
    pep517_args = get_maturin_pep517_args(config_settings)
    if pep517_args:
        command.extend(pep517_args)

    print("Running `{}`".format(" ".join(command)))
    try:
        _output = subprocess.check_output(command, env=_get_env())
    except subprocess.CalledProcessError as e:
        sys.stderr.write(f"Error running maturin: {e}\n")
        sys.exit(1)
    sys.stdout.buffer.write(_output)
    sys.stdout.flush()
    output = _output.decode(errors="replace")
    return output.strip().splitlines()[-1]


# Metadata for editable are the same as for a wheel
prepare_metadata_for_build_editable = prepare_metadata_for_build_wheel
