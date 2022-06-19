import sys
from pathlib import Path

import nox


def download_pyodide(session: nox.Session, pyodide_dir: Path) -> None:
    pyodide_dir.mkdir()

    PYODIDE_DEV = "https://pyodide-cdn2.iodide.io/dev/full/"
    pyodide_files = [
        "pyodide.js",
        "packages.json",
        "pyodide.asm.js",
        "pyodide.asm.data",
        "pyodide.asm.wasm",
        "pyodide_py.tar",
    ]
    with session.chdir(pyodide_dir):
        for file in pyodide_files:
            session.run("wget", "-q", PYODIDE_DEV + file, external=True)
        session.run("npm", "i", "node-fetch", external=True)


@nox.session(name="test-emscripten")
def test_emscripten(session: nox.Session):
    emscripten_dir = Path("./tests").resolve()
    pyodide_dir = emscripten_dir / "pyodide"
    if not pyodide_dir.exists():
        download_pyodide(session, pyodide_dir)

    test_crates = [
        "test-crates/pyo3-mixed",
    ]
    for crate in test_crates:
        crate = Path(crate).resolve()

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
