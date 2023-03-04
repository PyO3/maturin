# Migrating from older maturin versions

This guide can help you upgrade code through breaking changes from one maturin version to the next.
For a detailed list of all changes, see the [CHANGELOG](changelog.md).

## From 0.13.* to 0.14

### Remove support for specifying python package metadata in `Cargo.toml`

maturin 0.14 removed support for specifying python package metadata in `Cargo.toml`,
Python package metadata should be specified in the `project` section of `pyproject.toml` instead as [PEP 621](https://peps.python.org/pep-0621/) specifies.

### Deprecate `[tool.maturin.sdist-include]`

maturin 0.14 added `[tool.maturin.include]` and `[tool.maturin.exclude]`
to replace `[tool.maturin.sdist-include]` which was sdist only, the new options
can be configured to apply to sdist and/or wheel.

### macOS deployment target version defaults what `rustc` supports

If you don't set the `MACOSX_DEPLOYMENT_TARGET` environment variable,
maturin 0.14 will use the default target version quired from `rustc`, 
this may cause build issue for projects that depend on C/C++ code,
usually you can fix it by setting a correct `MACOSX_DEPLOYMENT_TARGET`, for example

```bash
export MACOSX_DEPLOYMENT_TARGET=10.9
```

### Deprecate `python-source` option in `Cargo.toml`

maturin 0.14 deprecated the `python-source` option in `Cargo.toml`,
use `[tool.maturin.python-source]` option in `pyproject.toml` instead.

## From 0.12.* to 0.13

### Drop support for Python 3.6

maturin 0.13 has dropped support for Python 3.6, to support Python 3.6 you can use the old 0.12 versions.

### Removed `--cargo-extra-args` and `--rustc-extra-args`

maturin 0.13 added most of the `cargo rustc` options so you can just use them directly,
for example `--cargo-extra-args="--no-default-features"` becomes `--no-default-features`.

To pass extra arguments to rustc, add them after `--`, 
for example use `maturin build -- -Clink-arg=-s` instead of `--rustc-extra-args="-Clink-arg=-s"`.

### Source distributions are not built by default

maturin 0.13 replaced `--no-sdist` with the new `--sdist` option in `maturin build` command,
source distributions are now only built when `--sdist` is specified.

### Only build wheels for current Python interpreter in `PATH` by default

maturin 0.13 no longer searches for Python interpreters by default and only build wheels for the current
Python interpreter (i.e. `python3`) in `PATH`.

To enable the old behavior, use the new `--find-interpreter` option.

### `--repository-url` only accepts full URL now

Previously `--repository-url` option in `maturin upload` and `maturin publish` commands accepts both
repository name and URL. maturin 0.13 changed `--repository-url` to only accept full URL and added a
new `--repository` for the repository name. This new behavior matches `twine upload`.
