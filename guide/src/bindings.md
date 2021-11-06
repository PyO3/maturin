# Bindings

Maturin supports several kinds of bindings, some of which are automatically
detected. You can also pass `-b` / `--bindings` command line option to manually
specify which bindings to use.

## `pyo3`

[pyo3](https://github.com/PyO3/pyo3) is Rust bindings for Python,
including tools for creating native Python extension modules.
It supports both CPython and PyPy.

maturin automatically detects pyo3 bindings when it's added as a dependency in `Cargo.toml`.

### `Py_LIMITED_API`/abi3

pyo3 bindings has `Py_LIMITED_API`/abi3 support, enable the `abi3` feature of the `pyo3` crate to use it:

```toml
pyo3 = { version = "0.14", features = ["abi3"] }
```

You may additionally specify a minimum Python version by using the `abi3-pyXX`
format for the pyo3 features, where `XX` is corresponds to a Python version.
For example `abi3-py37` will indicate a minimum Python version of 3.7.

> **Note**: Read more about abi3 support in [pyo3's
> documentation](https://pyo3.rs/latest/building_and_distribution.html#py_limited_apiabi3).

### Cross Compiling

pyo3 bindings has decent cross compilation support.
For manylinux support the [manylinux-cross](https://github.com/messense/manylinux-cross) docker images can be used.

> **Note**: Read more about cross compiling in [pyo3's
> documentation](https://pyo3.rs/latest/building_and_distribution.html#cross-compiling).

## `cffi`

Cffi wheels are compatible with all python versions including pypy. If `cffi`
isn't installed and python is running inside a virtualenv, maturin will install
it, otherwise you have to install it yourself (`pip install cffi`).

Maturin uses cbindgen to generate a header file for [supported Rust
types](https://github.com/eqrion/cbindgen/blob/master/docs.md#supported-types).
The header file can be customized by configuring cbindgen through a
`cbindgen.toml` file inside your project root. Aternatively you can use a build
script that writes a header file to `$PROJECT_ROOT/target/header.h`, like so:

```rust
use cbindgen;
use std::env;
use std::path::Path;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    let bindings = cbindgen::Builder::new()
        .with_no_includes()
        .with_language(cbindgen::Language::C)
        .with_crate(crate_dir)
        .generate()
        .unwrap();
    bindings.write_to_file(Path::new("target").join("header.h"));
}
```

Maturin uses the cbindgen-generated header to create a module that exposes `ffi` and
`lib` objects as attributes. See the [cffi docs](https://cffi.readthedocs.io/en/latest/using.html)
for more information on using these `ffi`/`lib` objects to call the Rust code
from Python.

> **Note**: Maturin _does not_ automatically detect `cffi` bindings. You _must_
> specify them via either command line with `-b cffi` or in `pyproject.toml`.

## `rust-cpython`

[rust-cpython](https://github.com/dgrunwald/rust-cpython) is Rust bindings for
the Python interpreter. Currently it only supports CPython.

Maturin automatically detects rust-cpython bindings when it's added as a
dependency in `Cargo.toml`.

## `bin`

Maturin also supports distributing binary applications written in Rust as
Python packages using the `bin` bindings. Binaries are packaged into the wheel
as "scripts" and are available on the user's `PATH` (e.g. in the `bin`
directory of a virtual environment) once installed.

> **Note**: Maturin _does not_ automatically detect `bin` bindings. You _must_
> specify them via either command line with `-b bin` or in `pyproject.toml`.
