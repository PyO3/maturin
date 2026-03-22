# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "nox",
# ]
# ///
import os
import json
import sys
from pathlib import Path
import tomllib

import nox

import urllib.request
import re

PYODIDE_VERSION = os.getenv("PYODIDE_VERSION", "0.29.0")
GITHUB_ACTIONS = os.getenv("GITHUB_ACTIONS")
GITHUB_ENV = os.getenv("GITHUB_ENV")
MSRV = tomllib.loads(Path("Cargo.toml").read_text())["package"]["rust-version"]


def append_to_github_env(name: str, value: str):
    if not GITHUB_ACTIONS or not GITHUB_ENV:
        return

    with open(GITHUB_ENV, "w+") as f:
        f.write(f"{name}={value}\n")


def get_latest_pyo3_version():
    url = "https://crates.io/api/v1/crates/pyo3"
    with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "maturin-nox-update"})) as response:
        data = json.loads(response.read().decode())
        return data["crate"]["max_stable_version"]


@nox.session(name="update-pyo3", python=False)
def update_pyo3(session: nox.Session):
    latest_version = get_latest_pyo3_version()
    session.log(f"Latest pyo3 version: {latest_version}")

    crates = ["pyo3", "pyo3-ffi", "pyo3-build-config"]
    # Update test crates and root Cargo.toml
    cargo_tomls = [p for p in Path(".").glob("**/Cargo.toml") if "pyo3-no-extension-module" not in str(p)]
    # Update templates
    templates = list(Path("src/templates").glob("*.j2"))

    for path in cargo_tomls + templates:
        content = path.read_text()
        changed = False
        for crate in crates:
            # Replace crate = "version"
            new_content = re.sub(
                rf'^(\s*{crate}\s*=\s*)"[0-9.]+"',
                rf'\1"{latest_version}"',
                content,
                flags=re.MULTILINE,
            )
            if new_content != content:
                content = new_content
                changed = True
            # Replace crate = { version = "version"
            new_content = re.sub(
                rf'^(\s*{crate}\s*=\s*\{{.*?version\s*=\s*)"[0-9.]+"',
                rf'\1"{latest_version}"',
                content,
                flags=re.MULTILINE,
            )
            if new_content != content:
                content = new_content
                changed = True

        if changed:
            session.log(f"Updating {path}")
            path.write_text(content)

    test_crate_dir = Path("./test-crates").resolve()
    crates_to_update = ["pyo3", "pyo3-ffi", "python3-dll-a"]
    for root, _, files in os.walk(test_crate_dir):
        if "pyo3-no-extension-module" in root:
            continue
        if "Cargo.lock" in files:
            cargo_lock_path = Path(root) / "Cargo.lock"
            with open(cargo_lock_path, "r") as lock_file:
                content = lock_file.read()
                for crate in crates_to_update:
                    if f'name = "{crate}"' in content:
                        with session.chdir(root):
                            session.run("cargo", f"+{MSRV}", "update", "-p", crate, external=True)


@nox.session(name="setup-pyodide", python=False)
def setup_pyodide(session: nox.Session):
    tests_dir = Path("./tests").resolve()
    with session.chdir(tests_dir):
        session.run(
            "npm",
            "i",
            "--no-save",
            f"pyodide@{PYODIDE_VERSION}",
            "prettier",
            external=True,
        )
        with session.chdir(tests_dir / "node_modules" / "pyodide"):
            session.run(
                "node",
                "../prettier/bin/prettier.cjs",
                "-w",
                "pyodide.asm.js",
                external=True,
            )
            with open("pyodide-lock.json") as f:
                emscripten_version = json.load(f)["info"]["platform"].split("_", 1)[1].replace("_", ".")
                append_to_github_env("EMSCRIPTEN_VERSION", emscripten_version)


@nox.session(name="test-emscripten", python=False)
def test_emscripten(session: nox.Session):
    tests_dir = Path("./tests").resolve()

    test_crates = [
        "test-crates/pyo3-pure",
        "test-crates/pyo3-mixed",
    ]
    for crate in test_crates:
        crate = Path(crate).resolve()
        ver = sys.version_info
        session.run("cargo", "build", external=True)
        session.run(
            tests_dir.parent / "target" / "debug" / "maturin",
            "build",
            "-m",
            str(crate / "Cargo.toml"),
            "--target",
            "wasm32-unknown-emscripten",
            "-i",
            f"python{ver.major}.{ver.minor}",
            env={"RUSTUP_TOOLCHAIN": "nightly"},
            external=True,
        )

        with session.chdir(tests_dir):
            session.run("node", "emscripten_runner.js", str(crate), external=True)
