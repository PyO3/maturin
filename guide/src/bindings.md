# Bindings

maturin supports several kind of bindings, some of them are automatically
detected. You can also pass `-b` / `--bindings` command line option to manually
specify it.

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

> **Note**: Read more about abi3 support in [pyo3's documentation](https://pyo3.rs/latest/building_and_distribution.html#py_limited_apiabi3).

### Cross Compiling

pyo3 bindings has decent cross compilation support.
For manylinux support the [manylinux-cross](https://github.com/messense/manylinux-cross) docker images can be used.

> **Note**: Read more about cross compiling in [pyo3's documentation](https://pyo3.rs/latest/building_and_distribution.html#cross-compiling).

## `cffi`

TODO

## `rust-cpython`

[rust-cpython](https://github.com/dgrunwald/rust-cpython) is Rust bindings for
the Python interperter. Currently it only supports CPython.

maturin automatically detects rust-cpython bindings when it's added as a dependency in `Cargo.toml`.

## `bin`

TODO
