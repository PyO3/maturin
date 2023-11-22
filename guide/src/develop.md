# Local Development

## `maturin develop` command

For local development, the `maturin develop` command can be used to quickly
build a package in debug mode by default and install it to virtualenv.

```
Usage: maturin develop [OPTIONS] [ARGS]...

Arguments:
  [ARGS]...
          Rustc flags

Options:
  -b, --bindings <BINDINGS>
          Which kind of bindings to use

          [possible values: pyo3, pyo3-ffi, rust-cpython, cffi, uniffi, bin]

      --strip
          Strip the library for minimum file size

  -E, --extras <EXTRAS>
          Install extra requires aka. optional dependencies

          Use as `--extras=extra1,extra2`

      --skip-install
          Skip installation, only build the extension module inplace

          Only works with mixed Rust/Python project layout

      --pip-path <PIP_PATH>
          Use a specific pip installation instead of the default one.

          This can be used to supply the path to a pip executable when the current virtualenv does
          not provide one.

  -q, --quiet
          Do not print cargo log messages

      --ignore-rust-version
          Ignore `rust-version` specification in packages

  -v, --verbose...
          Use verbose output (-vv very verbose/build.rs output)

      --color <WHEN>
          Coloring: auto, always, never

      --config <KEY=VALUE>
          Override a configuration value (unstable)

  -Z <FLAG>
          Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details

      --future-incompat-report
          Outputs a future incompatibility report at the end of the build (unstable)

  -h, --help
          Print help (see a summary with '-h')

Compilation Options:
  -r, --release
          Pass --release to cargo

  -j, --jobs <N>
          Number of parallel jobs, defaults to # of CPUs

      --profile <PROFILE-NAME>
          Build artifacts with the specified Cargo profile

      --target <TRIPLE>
          Build for the target triple

          [env: CARGO_BUILD_TARGET=]

      --target-dir <DIRECTORY>
          Directory for all generated artifacts

      --timings=<FMTS>
          Timing output formats (unstable) (comma separated): html, json

Feature Selection:
  -F, --features <FEATURES>
          Space or comma separated list of features to activate

      --all-features
          Activate all available features

      --no-default-features
          Do not activate the `default` feature

Manifest Options:
  -m, --manifest-path <PATH>
          Path to Cargo.toml

      --frozen
          Require Cargo.lock and cache are up to date

      --locked
          Require Cargo.lock is up to date

      --offline
          Run without accessing the network
```

## PEP 660 Editable Installs

Maturin supports [PEP 660](https://www.python.org/dev/peps/pep-0660/) editable installs since v0.12.0.
You need to add `maturin` to `build-system` section of `pyproject.toml` to use it:

```toml
[build-system]
requires = ["maturin>=1.0,<2.0"]
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
