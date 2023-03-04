# Project Layout

Maturin expects a particular project layout depending on the contents of the
package.

## Pure Rust project

For a pure Rust project, the structure is as expected and what you get from `cargo new`:

```
my-rust-project/
├── Cargo.toml
├── pyproject.toml  # required for maturin configuration
└── src
    ├── lib.rs  # default for library crates
    └── main.rs  # default for binary crates
```

Maturin will add a necessary `__init__.py` to the package when building the
wheel. For convenience, this file includes the following:

```python
from .my_project import *

__doc__ = my_project.__doc__
if hasattr(my_project, "__all__"):
    __all__ = my_project.__all__
```

such that the module functions may be called directly with:

```python
import my_project
my_project.foo()
```

rather than:

```python
from my_project import my_project
```

> **Note**: there is currently no way to tell maturin to include extra data (e.g.
`package_data` in setuptools) for a pure Rust project. Instead, consider using
the layout described below for the mixed Rust/Python project.

## Mixed Rust/Python project

To create a mixed Rust/Python project, add a directory with your package name
(i.e. matching `lib.name` in your `Cargo.toml`) to contain the Python source:

```
my-rust-and-python-project
├── Cargo.toml
├── my_project  # <<< add this directory and put Python code in here
│   ├── __init__.py
│   └── bar.py
├── pyproject.toml
├── README.md
└── src
    └── lib.rs
```

Note that in a mixed Rust/Python project, maturin _does not_ modify the
existing `__init__.py` in the root package, so now to import the rust module in
Python you must use:

```python
from my_project import my_project
```

You can modify `__init__.py` yourself (see above) if you would like to import
Rust functions from a higher-level namespace.

You can specify a different python source directory in `pyproject.toml` by setting `tool.maturin.python-source`, for example

**pyproject.toml**

```toml
[tool.maturin]
python-source = "python"
```

then the project structure would look like this:

```
my-rust-and-python-project
├── Cargo.toml
├── python
│   └── my_project
│       ├── __init__.py
│       └── bar.py
├── pyproject.toml
├── README.md
└── src
    └── lib.rs
```

> **Note**
>
> This structure is recommended to avoid [a common `ImportError` pitfall](https://github.com/PyO3/maturin/issues/490)


### Alternate Python source directory (src layout)

Having a directory with `package_name` in the root of the project can
occasionally cause confusion as Python allows importing local packages and
modules. A popular way to avoid this is with the `src`-layout, where the Python
package is nested within a `src` directory. Unfortunately this interferes with
the structure of a typical Rust project. Fortunately, Python is nor particular
about the name of the parent source directory.

maturin will detect the following src layout automatically:

```
my-rust-and-python-project
├── src  # put python code in src folder
│   └── my_project
│       ├── __init__.py
│       └── bar.py
├── pyproject.toml
├── README.md
└── rust # put rust code in rust folder
    |── Cargo.toml
    └── src
        └── lib.rs
```
#### Import Rust as a submodule of your project

If the Python module created by Rust has the same name as the Python package in a mixed Rust/Python project, IDEs might get confused. You might also want to discourage end users from using the Rust functions directly by giving it a different name, say '\_my_project'. This can be done by adding `name = <package name>.<rust pymodule name>` to the `[package.metadata.maturin]` in your `Cargo.toml`. For example:

```toml
[package.metadata.maturin]
name = "my_project._my_project"
```

You can then import your Rust module inside your Python source as follows:

```python
from my_project import _my_project
```

IDEs can then recognize the `_my_project` module as separate from your main Python source module. This allows for code completion of the types inside your Rust Python module for certain IDEs.


## Adding Python type information

To distribute typing information, you need to add:

* an empty marker file called `py.typed` in the root of the Python package
* inline types in Python files and/or `.pyi` "stub" files

In a pure Rust project, add type stubs in a `<module_name>.pyi` file in the
project root. Maturin will automatically include this file along with the
required `py.typed` file for you.

```
my-rust-project/
├── Cargo.toml
├── my_project.pyi  # <<< add type stubs for Rust functions in the my_project module here
├── pyproject.toml
└── src
    └── lib.rs
```

In a mixed Rust/Python project, additional files in the Python source dir (but
not in `.gitignore`) will be automatically included in the build outputs
(source distribution and/or wheel). Type information can be therefore added to
the root Python package directory as you might do in a pure Python package.
This requires you to add the `py.typed` marker file yourself.

```
my-project
├── Cargo.toml
├── python
│   └── my_project
│       ├── __init__.py
│       ├── py.typed  # <<< add this empty file
│       ├── my_project.pyi  # <<< add type stubs for Rust functions in the my_project module here
│       ├── bar.pyi  # <<< add type stubs for bar.py here OR type bar.py inline
│       └── bar.py
├── pyproject.toml
├── README.md
└── src
    └── lib.rs
```

## Data

You can add wheel data by creating a `<module_name>.data` folder or setting its location as `data` in pyproject.toml under `[tool.maturin]` or in Cargo.toml under `[project.metadata.maturin]`.

The data folder may have the following subfolder:

 * `data`: The contents of this folder will simply be unpacked into the virtualenv
 * `scripts`: Treated similar to entry points, files in there are installed as standalone executable
 * `headers`: For `.h` C header files
 * `purelib`: This also exists, but seems to be barely used
 * `platlib`: This also exists, but seems to be barely used

If you add a symlink in the data directory, we'll include the actual file so you have more flexibility.
