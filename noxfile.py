import sys
from pathlib import Path
import os

import nox

@nox.session(name="test-emscripten")
def test_emscripten(session: nox.Session):
    emscripten_dir = Path("./tests").resolve()

    test_crates = [
        "test-crates/pyo3-pure",
        "test-crates/pyo3-mixed",
    ]
    for crate in test_crates:
        crate = Path(crate).resolve()
        os.environ["RUSTFLAGS"] = "-C link-arg=-sWASM_BIGINT -Z print-link-args"
        ver = sys.version_info
        session.run(
            "cargo",
            "+nightly",
            "run",
            "build",
            "-m",
            str(crate / "Cargo.toml"),
            "--target",
            "wasm32-unknown-emscripten",
            "-i",
            f"python{ver.major}.{ver.minor}",
            external=True,
        )

        with session.chdir(emscripten_dir):
            session.run("node", "emscripten_runner.js", str(crate), external=True)
