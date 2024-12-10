# Import Hook

[maturin_import_hook](https://pypi.org/project/maturin-import-hook/) provides a mechanism to
automatically rebuild Maturin projects when they are imported by Python scripts.

This reduces friction when developing mixed Python/Rust codebases because edits made to Rust components take effect
automatically like edits to Python components do. Automatic rebuilding eliminates the possibility of Python code
using outdated rust components, which often leads to confusing behaviour.

The import hook can be configured to activate automatically as the interpreter starts or manually in individual scripts.
Only Maturin packages installed editable mode (`maturin develop` or `pip install -e`) are considered by the import hook.
When the build is up to date the overhead of the import hook is minimal.

The hook has some additional features such as importing stand-alone `.rs` files.
See [Features](#features) for a complete list.

## Installation

Install the package with:
```shell
$ pip install maturin_import_hook
```

Then install it into the current virtual environment with:
```shell
$ python -m maturin_import_hook site install
```
This will activate the hook automatically as the interpreter starts.
This command only has to be run once per virtual environment.
To use `site install`, ensure you have write access to `site-packages`.
Using a [virtual environment](https://docs.python.org/3/library/venv.html) is recommended rather than installing into
the system interpreter.

The managed site installation can be removed with:
```shell
$ python -m maturin_import_hook site uninstall
```

To manually activate the import hook only for specific scripts instead of installing into the virtual
environment see [Manual Activation](#manual-activation).

## Basic Configuration

The site installation can be customized. For example, to build in release mode:
```shell
$ python -m maturin_import_hook site install --args="--release"
```
The `--args` option accepts `maturin develop` arguments. For more options see `--help`.

The site installation can also be edited manually. Use `python -m maturin_import_hook site info` to locate the
`sitecustomize.py` file. See [Install Arguments](#install-arguments) for a list of arguments to `install()`.

To avoid unnecessary rebuilds caused by irrelevant file modifications, create empty `.maturin_hook_ignore` files to
manually ignore directories. Several directories and file types such as `target/` and `*.py` are ignored by default.
If more customization is needed, see [Custom File Searching](#custom-file-searching).

## CLI

The package provides a CLI for managing the build cache and site installation. For details, run:

```shell
python -m maturin_import_hook --help
```

* `site (info | install | uninstall)`
  * Manage import hook installation in [`sitecustomize.py`](#sitecustomize) of the
    active environment.
  * `install` options:
    * `--force`: Whether to overwrite any existing managed import hook installation.
    * `--(no-)-project-importer`: Whether to enable the project importer.
    * `--(no-)-rs-file-importer`: Whether to enable the rs file importer.
    * `--(no-)-detect-uv`: Whether to automatically detect and use the `--uv` flag.
    * `--args`: The arguments to pass to `maturin`.
    * `--user`: whether to install into `usercustomize.py` instead of `sitecustomize.py`.
* `cache (info | clear)`
  * Get info about the size and location of the build cache.
  * Manage the build cache of the active environment.
* `version`
  * Show version info of the import hook and associated tools. Useful for providing information to bug reports.


## Features

The import hook supports:

* All the binding types and project layouts supported by Maturin.
* All [CPython](https://www.python.org/) and [PyPy](https://pypy.org/) versions supported by Maturin.
* Windows, Linux and MacOS.
* Interactive environments such as [IPython](https://ipython.org/), [Jupyter](https://jupyter.org/) and
  [Python REPL](https://docs.python.org/3/tutorial/interpreter.html#interactive-mode).
* Importing multiple Maturin projects in the same script.
* Importing stand-alone `.rs` files that use `PyO3` bindings (see [Import Rust File](#import-rust-file)).
* `importlib.reload()` (currently unsupported on Windows).
* Detecting edits to path dependencies at any depth.
* Multiple environments with different interpreters using isolated build caches.
* Concurrent imports/builds from multiple scripts (useful for tools like [`pytest-xdist`](https://pypi.org/project/pytest-xdist/))
* Extensibility (see [Advanced Usage](#advanced-usage)).

## Manual Activation

To activate the import hook in a single Python script, call `install()` at the top of the script:

```python
import maturin_import_hook
maturin_import_hook.install()  # Must come first. Not active for imports above.

import my_rust_package  # An editable-installed Maturin project.

import foo.my_rust_script  # `foo/my_rust_script.rs` defines a pyo3 module.
```

The Maturin project importer and the Rust file importer can be used separately:

```python
from maturin_import_hook import project_importer
project_importer.install()

from maturin_import_hook import rust_file_importer
rust_file_importer.install()
```

The import hook can be configured to control its behaviour (see [Install Arguments](#install-arguments)).

The import hook is intended for use in development environments and not for
production environments, so any calls to `install()` should ideally be removed
(or disabled, see [Environment Variables](#environment-variables)) before reaching production.
This is another reason why the site installation is convenient.

The hook remains active across multiple Python modules so a single `install()` at the top of the main module is
sometimes sufficient. This can cause problems however, when the main module is not imported or not imported first,
such as in tests. `install()` can be called in each module but be aware that each call replaces the previous settings.

Tip: If you want to use non-defaults across multiple modules you can create a function like:
```python
import maturin_import_hook

_HOOK_INSTALLED = False

def install_maturin_hook() -> None:
    global _HOOK_INSTALLED
    if not _HOOK_INSTALLED:
        maturin_import_hook.install(
            ...  # your custom configuration here
        )
        _HOOK_INSTALLED = True
```
Then call `install_maturin_hook()` at the top of each module. This will ensure the custom options are used everywhere
without any code duplication.

## Import Rust File

When a `.rs` file is imported, a Maturin project with [pyo3](https://pyo3.rs/) bindings will be created for it in the
Maturin build cache and the resulting library will be built and loaded.

For example, `my_extension.rs`:
```rust
use pyo3::prelude::*;

#[pyfunction]
fn double(x: usize) -> usize { x * 2 }

#[pymodule]
fn my_extension(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(double, m)?)
}
```
The `#[pymodule]` must have the same name as the filename.
The version of `pyo3` is determined by `maturin new --bindings pyo3`.



## Environment Variables

The import hook can be disabled by setting `MATURIN_IMPORT_HOOK_ENABLED=0`. This can be used to disable
the import hook in production while leaving calls to `install()` in place.

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
also has extensive `DEBUG` level logging that would clutter application logs. So by not propagating, `DEBUG`
messages from the import hook are not shown even if the root logger has `DEBUG` level visible.

If you prefer, `maturin_import_hook.reset_logger()` can be called to undo the default configuration and propagate
the messages as normal.

When debugging issues with the import hook, you should first call `reset_logger()` then configure the root logger
to show `DEBUG` messages. You can also run with the environment variable `RUST_LOG=maturin=debug` to get more
information from Maturin.

```python
import logging
logging.basicConfig(format='%(asctime)s %(name)s [%(levelname)s] %(message)s', level=logging.DEBUG)
import maturin_import_hook
maturin_import_hook.reset_logger()
maturin_import_hook.install()
```

## Background

### Import Hook

An [import hook](https://docs.python.org/3/reference/import.html#import-hooks) is a class added to
[sys.meta_path](https://docs.python.org/3/library/sys.html#sys.meta_path) that is invoked by the Python interpreter
whenever it encounters an `import` statement (or import via other means). `find_spec()` is called for each hook until
one returns a `ModuleSpec` indicating that it found the module. If a hook cannot handle a particular import it
returns `None`. Import hooks can be used to create modules on demand (e.g. the `.rs` importer) or trigger
side-effects with imports (e.g. the project importer).

[MaturinProjectImporter](https://github.com/PyO3/maturin-import-hook/blob/main/src/maturin_import_hook/project_importer.py):

1. Detects when an import corresponds to an editable installed Maturin project.
2. Determines if the current build of the project is up to date.
3. Rebuilds the project if necessary.

[MaturinRustFileImporter](https://github.com/PyO3/maturin-import-hook/blob/main/src/maturin_import_hook/rust_file_importer.py):

1. Searches `sys.path` for an `.rs` file matching the import
  (e.g. for `import foo.bar`, the hook will look for `foo/bar.rs` at each search path).
2. Creates a temporary Maturin project for the `.rs` file or re-uses the project if it already exists.
3. Rebuilds the project if necessary.

The above steps are a simplification as supporting `importlib.reload()` requires more complex logic.
See [reloading.md](https://github.com/PyO3/maturin-import-hook/blob/main/docs/reloading.md) for more details.

### Sitecustomize

If a [sitecustomize.py](https://docs.python.org/3/library/site.html) file exists in the `site-packages` directory of
a Python installation it is loaded automatically by the Python interpreter as it starts unless the `-S` flag is
passed to it. This makes `sitecustomize.py` a convenient place to activate the import hook.


## Advanced Usage

### Install Arguments

The arguments to `install()` are ([source](https://github.com/PyO3/maturin-import-hook/blob/main/src/maturin_import_hook/__init__.py)):

```python
"""
enable_project_importer: enable the hook for automatically rebuilding editable installed maturin projects

enable_rs_file_importer: enable the hook for importing .rs files as though they were regular python modules

enable_reloading: enable workarounds to allow the extension modules to be reloaded with `importlib.reload()`

settings: settings corresponding to flags passed to maturin.

build_dir: where to put the compiled artifacts. defaults to `$MATURIN_BUILD_DIR`,
    `sys.exec_prefix / 'maturin_build_cache'` or
    `$HOME/.cache/maturin_build_cache/<interpreter_hash>` in order of preference

force_rebuild: whether to always rebuild and skip checking whether anything has changed

lock_timeout_seconds: a lock is required to prevent projects from being built concurrently.
    If the lock is not released before this timeout is reached the import hook stops waiting and aborts.
    A value of None means that the import hook will wait for the lock indefinitely.

show_warnings: whether to show compilation warnings

file_searcher: an object used to find source and installed project files that are used to determine whether
    a project has changed and needs to be rebuilt

enable_automatic_install: whether to install detected packages using the import hook even if they
    are not already installed into the virtual environment or are installed in non-editable mode.
"""
```
### Subclassing

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

### Custom File Searching

The `file_searcher` argument to `install()` can be used to customize which files should be included/excluded when
deciding whether a package needs to be rebuilt:

```python
from collections.abc import Iterator
from pathlib import Path
from maturin_import_hook.project_importer import ProjectFileSearcher, install

class CustomFileSearcher(ProjectFileSearcher):
    def get_source_paths(
        self,
        project_dir: Path,
        all_path_dependencies: list[Path],
        installed_package_root: Path,
    ) -> Iterator[Path]: ...

    def get_installation_paths(self, installed_package_root: Path) -> Iterator[Path]: ...

install(file_searcher=CustomFileSearcher())
```

See `maturin_import_hook.project.DefaultProjectFileSearcher` for the default list of include/exclude criteria.

The class variables of `DefaultProjectFileSearcher` can be edited **before** calling `install()` to make simple
customizations.
