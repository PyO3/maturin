# Sphinx Documentation Integration

Sphinx is a popular documentation generator in Python community.
It's commonly used together with services like [Read The Docs](https://readthedocs.org/)
which automates documentation building, versioning, and hosting for you.

Usually in a pure Python project setting up Sphinx is easy, just follow the
[quick start](https://www.sphinx-doc.org/en/master/usage/quickstart.html) of
Sphinx documentation is enough. But it can get complicated when Rust
based Python extension modules are involved.

With maturin, first you need to make sure you have added a `pyproject.toml` and
properly configured it to build source distributions, for example a minimal configuration below:

```toml
[build-system]
requires = ["maturin>=0.12,<0.13"]
build-backend = "maturin"
```

With this `pip install .` should work when invoked in the project directory.

## Read The Docs Integration

To build documentation on [Read The Docs](https://readthedocs.org/), you need
to tell it to install the Rust compiler and Python interpreter in its build environment,
you can do it by adding a `.readthedocs.yaml` in your project root:

```yaml
# https://docs.readthedocs.io/en/stable/config-file/v2.html#supported-settings

version: 2

sphinx:
  builder: html

build:
  os: "ubuntu-20.04"
  tools:
    python: "3.9"
    rust: "1.55"

python:
  install:
    - method: pip
      path: .
```

If you're using a mixed Rust/Python project layout, make sure you didn't add the
Python project path to `sys.path` in `conf.py` of Sphinx. Read The Docs
doesn't install your project in editable mode, adding it to `sys.path` will make
your project fail to import which breaks documentation generation.

If you need to install a specific version of Sphinx or adding Sphinx
themes/extensions, you can change the `python.install` section a bit to add an
extra installation step, for example:

```yaml
python:
  install:
    - requirements: docs/requirements.txt
    - method: pip
      path: .
```

In `docs/requirements.txt` you can add some Python package requirements you
needs build the documentation.

## Netlify Integration

[Netlify](https://www.netlify.com/) is another popular automated site hosting
service that can be used with Sphinx and other documentation tools.

Netlify configuration can be specified in a `.netlify.toml` file. Assuming your
Sphinx documentation files are placed in `docs/` directory, a minimal
configurationfor maturin based project can be:

```toml
[build]
  base = "docs"
  publish = "_build/html"
  command = "maturin develop -m ../Cargo.toml && make html"
```

You also need to add a `rust-toolchain` file at `docs/rust-toolchain` which netlify
will use to install the specified Rust toolchain that maturin needs to compile
your project.

For Sphinx which is written in Python to run you need to add a `runtime.txt` at
`docs/runtime.txt`, its content should be a Python interpreter version for
example `3.8`. Then a `requirements.txt` file at `docs/requirements.txt` is
needed to install Sphinx and its dependencies, you can generate one by:

```bash
python3 -m venv venv
source venv/bin/activate
python3 -m pip install sphinx
python3 -m pip freeze > docs/requirements.txt
```
