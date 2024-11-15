# Import Hook

[maturin_import_hook](https://pypi.org/project/maturin-import-hook/) is a package that provides a python import hook to
automatically rebuild maturin projects when they are imported.

This reduces friction when developing mixed python/rust codebases because changes made to rust
components take effect automatically like changes to python components do.

For `import` statements to trigger rebuilds, the hook must to be active (by calling `install()` or installing site-wide) and
the maturin project being imported must be installed in editable mode (eg with `maturin develop` or `pip install -e`).
Rebuilds are only triggered if the source code has changed, so the overhead is small if everything is up-to-date.

The hook also adds support for importing stand-alone `.rs` files by creating and building temporary maturin projects
for them.

## Installation

Run the following commands to install the package and optionally configure the hook to activate automatically when
starting the interpreter.

```shell
pip install maturin_import_hook
python -m maturin_import_hook site install
```

In order to use `site install`, you must have write access to `site-packages`. It is recommended to use a
[virtual environment](https://docs.python.org/3/library/venv.html) instead of installing into the system interpreter.

Alternatively, instead of using `site install`, put calls to `maturin_import_hook.install()` into any script where you
want to use the import hook.

## Usage

If the hook is installed site-wide, no code changes are required! just import a maturin project like normal and it will
rebuild when necessary.

If the hook is not installed site-wide, call `install()` like so:

```python
# install the import hook with default settings.
# can be skipped if installed site-wide (see above).
# must be called before importing any maturin project.
import maturin_import_hook
maturin_import_hook.install()

# when a maturin package that is installed in editable mode is imported,
# that package will be automatically recompiled if necessary.
import my_rust_package

# when a .rs file is imported a project will be created for it in the
# maturin build cache and the resulting library will be loaded.
#
# assuming subpackage/my_rust_script.rs defines a pyo3 module:
import subpackage.my_rust_script
```

The maturin project importer and the rust file importer can be used separately

```python
from maturin_import_hook import rust_file_importer
rust_file_importer.install()

from maturin_import_hook import project_importer
project_importer.install()
```

The import hook can be configured to control its behaviour

```python
import maturin_import_hook
from maturin_import_hook.settings import MaturinSettings

maturin_import_hook.install(
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

The import hook is intended for use in development environments and not for
production environments, so any calls to `install()` should ideally be removed before reaching production. This is
another reason why installing site-wide is convenient.

## Features

The import hook is fairly robust and supports the following:

* Supports all the binding types and project layouts supported by maturin.
* Supports importing multiple maturin projects in the same script.
* Supports importing stand-alone `.rs` files that use `PyO3` bindings.
* Supports `importlib.reload()` (currently not supported on Windows).
* Detects source code changes of local path dependencies, not just the top-level project.
* Can be used by multiple environments at once including with different interpreter versions.
  Each environment has a separate build cache.
* Handles multiple scripts attempting to import/build packages simultaneously.
  Each build cache is protected with an exclusive lock. (One case where this is useful is tests using
  [`pytest-xdist`](https://pypi.org/project/pytest-xdist/)).
* Extensible (see [Advanced Usage](#advanced-usage) below)

## CLI

The package provides a CLI interface for getting information such as the location and size of the build cache and
managing the installation into [`sitecustomize.py`](https://docs.python.org/3/library/site.html). For details, run:

```shell
python -m maturin_import_hook --help
```

* `site (info | install | uninstall)`
  * Manage import hook installation in [`sitecustomize.py`](https://docs.python.org/3/library/site.html) of the active environment.
* `cache (info | clear)`
  * Manage the build cache of the active environment.
* `version`
  * Show version info of the import hook and associated tools. Useful for providing information to bug reports.

## Environment Variables

The import hook can be disabled by setting `MATURIN_IMPORT_HOOK_ENABLED=0`. This can be used to disable
the import hook in production if you want to leave calls to `install()` in place.

Build files will be stored in an appropriate place for the current system but can be overridden
by setting `MATURIN_BUILD_DIR`. These files can be deleted without causing any issues (unless a build is in progress).
The precedence for storing build files is:

* `MATURIN_BUILD_DIR`
  * (Each environment will store its cache in a subdirectory of the given path).
* `<virtualenv_dir>/maturin_build_cache`
* `<system_cache_dir>/maturin_build_cache`
  * e.g. `~/.cache/maturin_build_cache` on POSIX.

See the location being used with the CLI: `python -m maturin_import_hook cache info`

## Logging

By default, the `maturin_import_hook` logger does not propagate to the root logger. This is so that `INFO` level
messages are shown without having to configure logging (`INFO` level is normally not visible). The import hook
also has extensive `DEBUG` level logging that generally would be more noise than useful. So by not propagating, `DEBUG`
messages from the import hook are not shown even if the root logger has `DEBUG` level visible.

If you prefer, `maturin_import_hook.reset_logger()` can be called to undo the default configuration and propagate
the messages as normal.

When debugging issues with the import hook, you should first call `reset_logger()` then configure the root logger
to show `DEBUG` messages. You can also run with the environment variable `RUST_LOG=maturin=debug` to get more
information from maturin.

```python
import logging
logging.basicConfig(format='%(asctime)s %(name)s [%(levelname)s] %(message)s', level=logging.DEBUG)
import maturin_import_hook
maturin_import_hook.reset_logger()
maturin_import_hook.install()
```

## Advanced Usage

The import hook classes can be subclassed to further customize to specific use cases.
For example settings can be configured per-project or loaded from configuration files.

```python
import sys
from pathlib import Path
from maturin_import_hook.settings import MaturinSettings
from maturin_import_hook.project_importer import MaturinProjectImporter

class CustomImporter(MaturinProjectImporter):
    def get_settings(self, module_path: str, source_path: Path) -> MaturinSettings:
        return MaturinSettings(
            release=True,
            strip=True,
            # ...
        )

sys.meta_path.insert(0, CustomImporter())
```
