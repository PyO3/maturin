import os
import json
import sys
from pathlib import Path

import nox


PYODIDE_VERSION = os.getenv("PYODIDE_VERSION", "0.29.0")
GITHUB_ACTIONS = os.getenv("GITHUB_ACTIONS")
GITHUB_ENV = os.getenv("GITHUB_ENV")
MSRV = "1.83.0"


def append_to_github_env(name: str, value: str):
    if not GITHUB_ACTIONS or not GITHUB_ENV:
        return

    with open(GITHUB_ENV, "w+") as f:
        f.write(f"{name}={value}\n")


@nox.session(name="update-pyo3", python=False)
def update_pyo3(session: nox.Session):
    # TODO: support updating major and minor versions by editing Cargo.toml first
    test_crate_dir = Path("./test-crates").resolve()
    crates_to_update = ["pyo3", "pyo3-ffi", "python3-dll-a"]
    for root, _, files in os.walk(test_crate_dir):
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
