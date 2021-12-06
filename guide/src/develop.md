# Local Development

## `maturin develop` command

For local development, the `maturin develop` command can be used to quickly
build a package in debug mode by default and install it to virtualenv.

```
USAGE:
    maturin develop [FLAGS] [OPTIONS]

FLAGS:
    -h, --help
            Prints help information

        --release
            Pass --release to cargo

        --strip
            Strip the library for minimum file size

    -V, --version
            Prints version information


OPTIONS:
    -b, --binding-crate <binding-crate>
            Which kind of bindings to use. Possible values are pyo3, rust-cpython, cffi and bin

        --cargo-extra-args <cargo-extra-args>...
            Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`

            Use as `--cargo-extra-args="--my-arg"`
    -E, --extras <extras>
            Install extra requires aka. optional dependencies

            Use as `--extras=extra1,extra2`
    -m, --manifest-path <manifest-path>
            The path to the Cargo.toml [default: Cargo.toml]

        --rustc-extra-args <rustc-extra-args>...
            Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`

            Use as `--rustc-extra-args="--my-arg"`
```

## PEP 660 Editable Installs

Maturin supports [PEP 660](https://www.python.org/dev/peps/pep-0660/) editable installs since v0.12.0.
You need to add `maturin` to `build-system` section of `pyproject.toml` to use it:

```toml
[build-system]
requires = ["maturin>=0.12,<0.13"]
build-backend = "maturin"
```

Editable installs right now is only useful in mixed Rust/Python project so you
don't have to recompile and reinstall when only Python source code changes. For
example when using pip you can make an editable installation with

```bash
pip install -e .
```

Then Python source code changes will take effect immediately.

## Import Hook

Starting from v0.12.4, the [Python maturin package](https://pypi.org/project/maturin/) provides
a Python import hook to allow quickly build and load a Rust module into Python.

It supports pure Rust and mixed Rust/Python project layout as well as a
standalone `.rs` file.

```python
from maturin import import_hook

# install the import hook with default settings
import_hook.install()
# or you can specify bindings
import_hook.install(bindings="pyo3")
# and build in release mode instead of the default debug mode
import_hook.install(release=True)

# now you can start importing your Rust module
import pyo3_pure
```
