# Bindings

Maturin supports several kinds of bindings, some of which are automatically
detected. You can also pass `-b` / `--bindings` command line option to manually
specify which bindings to use.

## `pyo3`

[pyo3](https://github.com/PyO3/pyo3) is Rust bindings for Python,
including tools for creating native Python extension modules.
It supports CPython, PyPy, and GraalPy.

maturin automatically detects pyo3 bindings when it's added as a dependency in `Cargo.toml`.

### `Py_LIMITED_API`/abi3

pyo3 bindings has `Py_LIMITED_API`/abi3 support, enable the `abi3` feature of the `pyo3` crate to use it:

```toml
pyo3 = { version = "0.26", features = ["abi3"] }
```

You may additionally specify a minimum Python version by using the `abi3-pyXX`
format for the pyo3 features, where `XX` is corresponds to a Python version.
For example `abi3-py37` will indicate a minimum Python version of 3.7.

> **Note**: Read more about abi3 support in [pyo3's
> documentation](https://pyo3.rs/latest/building-and-distribution#py_limited_apiabi3).

### Cross Compiling

pyo3 bindings has decent cross compilation support.
For manylinux support the [manylinux-cross](https://github.com/rust-cross/manylinux-cross) docker images can be used.

> **Note**: Read more about cross compiling in [pyo3's
> documentation](https://pyo3.rs/latest/building-and-distribution#cross-compiling).

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

Maturin automatically detect `cffi` bindings, but only if there is no pyo3
dependency. You can specify `cffi` explicitly via either command line with
`-b cffi` or in `pyproject.toml`.

## `bin`

Maturin also supports distributing binary applications written in Rust as
Python packages using the `bin` bindings. Binaries are packaged into the wheel
as "scripts" and are available on the user's `PATH` (e.g. in the `bin`
directory of a virtual environment) once installed.

Maturin automatically detect `bin` bindings, but only if there
is only a binary target and no pyo3 dependency or cdylib target. You can
specify `bin` explicitly via either command line with `-b bin` or in `pyproject.toml`.

### Both binary and library?

Shipping both a binary and library would double the size of your wheel. Consider instead exposing a CLI function in the library and using a Python entrypoint:

```rust
#[pyfunction]
fn print_cli_args(py: Python) -> PyResult<()> {
    // This one includes python and the name of the wrapper script itself, e.g.
    // `["/home/ferris/.venv/bin/python", "/home/ferris/.venv/bin/print_cli_args", "a", "b", "c"]`
    println!("{:?}", env::args().collect::<Vec<_>>());
    // This one includes only the name of the wrapper script itself, e.g.
    // `["/home/ferris/.venv/bin/print_cli_args", "a", "b", "c"])`
    println!(
        "{:?}",
        py.import("sys")?
            .getattr("argv")?
            .extract::<Vec<String>>()?
    );
    Ok(())
}

#[pymodule]
fn my_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(print_cli_args))?;

    Ok(())
}
```

In pyproject.toml:

```toml
[project.scripts]
print_cli_args = "my_module:print_cli_args"
```

## `uniffi`

uniffi bindings use [uniffi-rs](https://mozilla.github.io/uniffi-rs/) to generate Python `ctypes` bindings
from an interface definition file. uniffi wheels are compatible with all python versions including pypy.
