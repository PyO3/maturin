# Python Project Metadata

maturin supports [PEP 621](https://www.python.org/dev/peps/pep-0621/),
you can specify python package metadata in `pyproject.toml`.
maturin merges metadata from `Cargo.toml` and `pyproject.toml`, `pyproject.toml` take precedence over `Cargo.toml`.

Here is a `pyproject.toml` example from PEP 621 for reference purpose:

```toml
[project]
name = "spam"
version = "2020.0.0"
description = "Lovely Spam! Wonderful Spam!"
readme = "README.rst"
requires-python = ">=3.8"
license = {file = "LICENSE.txt"}
keywords = ["egg", "bacon", "sausage", "tomatoes", "Lobster Thermidor"]
authors = [
  {email = "hi@pradyunsg.me"},
  {name = "Tzu-Ping Chung"}
]
maintainers = [
  {name = "Brett Cannon", email = "brett@python.org"}
]
classifiers = [
  "Development Status :: 4 - Beta",
  "Programming Language :: Python"
]

dependencies = [
  "httpx",
  "gidgethub[httpx]>4.0.0",
  "django>2.1; os_name != 'nt'",
  "django>2.0; os_name == 'nt'"
]

[project.optional-dependencies]
test = [
  "pytest < 5.0.0",
  "pytest-cov[all]"
]

[project.urls]
homepage = "example.com"
documentation = "readthedocs.org"
repository = "github.com"
changelog = "github.com/me/spam/blob/master/CHANGELOG.md"

[project.scripts]
spam-cli = "spam:main_cli"

[project.gui-scripts]
spam-gui = "spam:main_gui"

[project.entry-points."spam.magical"]
tomatoes = "spam:main_tomatoes"
```

## Add Python dependencies

To specify python dependencies, add a list `dependencies` in a `[project]` section in the `pyproject.toml`. This list is equivalent to `install_requires` in setuptools:

```toml
[project]
name = "my-project"
dependencies = ["flask~=1.1.0", "toml==0.10.0"]
```

## Add console scripts

Pip allows adding so called console scripts, which are shell commands that execute some function in you program. You can add console scripts in a section `[project.scripts]`.
The keys are the script names while the values are the path to the function in the format `some.module.path:class.function`, where the `class` part is optional. The function is called with no arguments. Example:

```toml
[project.scripts]
get_42 = "my_project:DummyClass.get_42"
```

## Add trove classifiers

You can also specify [trove classifiers](https://pypi.org/classifiers/) under `project.classifiers`:

```toml
[project]
name = "my-project"
classifiers = ["Programming Language :: Python"]
```

## Add SPDX license expressions

A practical string value for the license key has been purposefully left out by PEP 621
to allow for a future PEP to specify support for
[SPDX](https://spdx.org/licenses/) expressions.

To use SPDX license expressions, you can specify it in `Cargo.toml` instead:

```toml
[package]
name = "my-project"
license = "MIT OR Apache-2.0"
```
