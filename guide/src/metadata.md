# Python Project Metadata

maturin supports [PEP 621](https://www.python.org/dev/peps/pep-0621/),
you can specify python package metadata in `pyproject.toml`.
maturin merges metadata from `Cargo.toml` and `pyproject.toml`, `pyproject.toml` take precedence over `Cargo.toml`.

## Add Python dependencies

To specify python dependencies, add a list `dependencies` in a `[project]` section in the `pyproject.toml`. This list is equivalent to `install_requires` in setuptools:

```toml
[project]
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

You can also specify [trove classifiers](https://pypi.org/classifiers/) in your Cargo.toml under `project.classifiers`:

```toml
[project]
classifiers = ["Programming Language :: Python"]
```
