# Migrating from older maturin versions

This guide can help you upgrade code through breaking changes from one maturin version to the next.
For a detailed list of all changes, see the [CHANGELOG](changelog.md).

## From 1.9.5 to 1.9.6

### Editable installs default to debug builds

Editable installs (`pip install -e .`) now default to the `dev` (debug) profile instead of `release` to speed up development workflows. The `maturin develop` command is unaffected. To restore the old behavior, you can explicitly set `profile = "release"` in the `[tool.maturin]` section of your `pyproject.toml`.

## From 0.14.* to 0.15

### Build with `--no-default-features` by default when bootstrapping from sdist

When bootstrapping maturin from sdist, maturin 0.15 will build with `--no-default-features` by default,
which means that for distro packaging, you might want to set the environment variable `MATURIN_SETUP_ARGS="--features full,rustls"` to enable full features.

### Remove `[tool.maturin.sdist-include]`

Use `[tool.maturin.include]` option instead.

### Remove `[package.metadata.maturin]` from `Cargo.toml`

Package metadata is now specified in `[tool.maturin]` section of `pyproject.toml` instead of `Cargo.toml`.
Note that the replacement for `package.metadata.maturin.name` is `tool.maturin.module-name`.

### Require `uniffi-bindgen` CLI to building `uniffi` bindings

maturin 0.15 requires `uniffi-bindgen` CLI to build `uniffi` bindings,
you can install it with `pip install uniffi-bindgen`.

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
maturin 0.14 will use the default target version acquired from `rustc`,
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
