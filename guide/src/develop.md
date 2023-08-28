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

  -r, --release
          Pass --release to cargo

      --strip
          Strip the library for minimum file size

  -E, --extras <EXTRAS>
          Install extra requires aka. optional dependencies

          Use as `--extras=extra1,extra2`

      --skip-install
          Skip installation, only build the extension module inplace

          Only works with mixed Rust/Python project layout

  -q, --quiet
          Do not print cargo log messages

  -j, --jobs <N>
          Number of parallel jobs, defaults to # of CPUs

      --profile <PROFILE-NAME>
          Build artifacts with the specified Cargo profile

  -F, --features <FEATURES>
          Space or comma separated list of features to activate

      --all-features
          Activate all available features

      --no-default-features
          Do not activate the `default` feature

      --target <TRIPLE>
          Build for the target triple

          [env: CARGO_BUILD_TARGET=]

      --target-dir <DIRECTORY>
          Directory for all generated artifacts

  -m, --manifest-path <PATH>
          Path to Cargo.toml

      --ignore-rust-version
          Ignore `rust-version` specification in packages

  -v, --verbose...
          Use verbose output (-vv very verbose/build.rs output)

      --color <WHEN>
          Coloring: auto, always, never

      --frozen
          Require Cargo.lock and cache are up to date

      --locked
          Require Cargo.lock is up to date

      --offline
          Run without accessing the network

      --config <KEY=VALUE>
          Override a configuration value (unstable)

  -Z <FLAG>
          Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details

      --timings=<FMTS>
          Timing output formats (unstable) (comma separated): html, json

      --future-incompat-report
          Outputs a future incompatibility report at the end of the build (unstable)

  -h, --help
          Print help (see a summary with '-h')
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

# when a rust package that is installed in editable mode is imported,
# that package will be automatically recompiled if necessary.
import pyo3_pure

# when a .rs file is imported a project will be created for it in the
# maturin build cache and the resulting library will be loaded
import subpackage.my_rust_script
```

The maturin project importer and the rust file importer can be used separately
```python
from maturin.import_hook import rust_file_importer
rust_file_importer.install()
from maturin.import_hook import project_importer
project_importer.install()
```

The import hook can be configured to control its behaviour
```python
from maturin import import_hook
from maturin.import_hook.settings import MaturinSettings

import_hook.install(
    enable_project_importer=True,
    enable_rs_file_importer=True,
    settings=MaturinSettings(
        release=True,
        strip=True,
        # ...
    ),
    show_warnings=True,
    # ...
)
```

Custom settings providers can be used to override settings of particular projects
or implement custom logic such as loading settings from configuration files
```python
from pathlib import Path
from maturin import import_hook
from maturin.import_hook.settings import MaturinSettings, MaturinSettingsProvider

class CustomSettings(MaturinSettingsProvider):
    def get_settings(self, module_path: str, source_path: Path) -> MaturinSettings:
        return MaturinSettings(
            release=True,
            strip=True,
            # ...
        )

import_hook.install(
    enable_project_importer=True,
    enable_rs_file_importer=True,
    settings=CustomSettings(),
    show_warnings=True,
    # ...
)
```

Since the import hook is intended for use in development environments and not for
production environments, it may be a good idea to put the call to `import_hook.install()`
into `site-packages/sitecustomize.py` of your development virtual environment
([documentation](https://docs.python.org/3/library/site.html)). This will
enable the hook for every script run by that interpreter without calling `import_hook.install()`
in every script, meaning the scripts do not need alteration before deployment.


The import hook internals can be examined by configuring the root logger and
calling `reset_logger` to propagate messages from the `maturin.import_hook` logger
to the root logger. You can also run with the environment variable `RUST_LOG=maturin=debug`
to get more information from maturin.
```python
import logging
logging.basicConfig(format='%(name)s [%(levelname)s] %(message)s', level=logging.DEBUG)
from maturin import import_hook
import_hook.reset_logger()
import_hook.install()
```
