import os
import json
import sys
from pathlib import Path

import nox


PYODIDE_VERSION = os.getenv("PYODIDE_VERSION", "0.21.3")
GITHUB_ACTIONS = os.getenv("GITHUB_ACTIONS")
GITHUB_ENV = os.getenv("GITHUB_ENV")


def append_to_github_env(name: str, value: str):
    if not GITHUB_ACTIONS or not GITHUB_ENV:
        return

    with open(GITHUB_ENV, "w+") as f:
        f.write(f"{name}={value}\n")


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
                "../prettier/bin-prettier.js",
                "-w",
                "pyodide.asm.js",
                external=True,
            )
            with open("repodata.json") as f:
                emscripten_version = (
                    json.load(f)["info"]["platform"].split("_", 1)[1].replace("_", ".")
                )
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
