# Migrating from older maturin versions

This guide can help you upgrade code through breaking changes from one maturin version to the next.
For a detailed list of all changes, see the [CHANGELOG](changelog.md).

## From 0.12.* to 0.13

### Drop support for Python 3.6

maturin 0.13 has dropped support for Python 3.6, to support Python 3.6 you can use the old 0.12 versions.

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
