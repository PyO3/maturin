# Configuration

## Configuration format

You can configure maturin in `tool.maturin` section of [`pyproject.toml`](https://packaging.python.org/en/latest/specifications/pyproject-toml/#arbitrary-tool-configuration-the-tool-table).

## Configuration keys

### Cargo options

```toml
[tool.maturin]
# Build artifacts with the specified Cargo profile
profile = "release"
# For "editable" builds, use the specified Cargo profile,
# e.g. to use "dev" builds for local development
# (uses `profile` if this key is not set)
editable-profile = "release"
# List of features to activate
features = ["foo", "bar"]
# Features can also be conditional on the target Python version
# using PEP 440 version specifiers:
# features = [
#   "always-on-feature",
#   { feature = "pyo3/abi3-py311", python-version = ">=3.11" },
#   { feature = "pyo3/abi3-py38", python-version = "<3.11" },
# ]
# Activate all available features
all-features = false
# Do not activate the `default` feature
no-default-features = false
# Cargo manifest path
manifest-path = "Cargo.toml"
# Require Cargo.lock and cache are up to date
frozen = false
# Require Cargo.lock is up to date
locked = false
# Override a configuration value (unstable)
config = []
# Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details
unstable-flags = []
# Extra arguments that will be passed to rustc as `cargo rustc [...] -- [...] [arg1] [arg2]`
rustc-args = []
```

These are `cargo` build options, refer Cargo documentation [here](https://doc.rust-lang.org/cargo/commands/cargo-rustc.html).

### maturin options

```toml
[tool.maturin]
# Python import name of the built extension module.
# Accepts dotted names like "my_package._native" so the Rust extension is
# installed as a submodule (useful for mixed Python/Rust projects).
# See the project layout docs for a full example.
module-name = "my_package._native"
# Include additional files
include = []
# Exclude files
exclude = []
# Bindings type
bindings = "pyo3"
# Control the platform tag and PyPI compatibility
compatibility = "pypi"
# auditwheel mode, possible values are repair, check and skip
auditwheel = "repair"
# Don't check for manylinux compliance, deprecated in favor of auditwheel = "skip"
skip-auditwheel = false
# Python source directory
python-source = "src"
# Python packages to include
python-packages = ["foo", "bar"]
# Path to the wheel data directory (see the Data section of the project layout
# docs). Defaults to looking for `<module-name>.data` next to the project root
# when that directory exists. Relative paths are resolved from the project root.
data = "my_package.data"
# Strip the library for minimum file size
strip = true
# Source distribution generator,
# supports cargo (default) and git.
sdist-generator = "cargo"
# Include the Windows import library (.dll.lib or .dll.a) in the wheel.
# This is useful when distributing shared libraries that other programs
# need to link against at compile time.
include-import-lib = false
# Use base Python executable instead of venv Python executable in PEP 517 build.
#
# This can help avoid unnecessary rebuilds, as the Python executable does not change
# every time. It should not be set when the sdist build requires packages installed
# in venv. This can also be set with the `MATURIN_PEP517_USE_BASE_PYTHON` environment
# variable.
use-base-python = false
# Shell command run during Profile-Guided Optimization (PGO) profile generation.
# Required when building with `--pgo` / `MATURIN_PGO`. Executed in a temporary
# virtualenv with the instrumented wheel installed.
# Example: "python -m pytest tests/benchmarks"
pgo-command = "python -m pytest tests/benchmarks"
# Select which Cargo compile targets to build when the crate defines more than
# one matching target. Each entry is an object with a required `name` (as in
# Cargo.toml) and an optional `kind` (`bin`, `cdylib`, `dylib`, `lib`, `rlib`,
# or `staticlib`). This is unrelated to `[tool.maturin.target.<triple>]` below.
# targets = [
#   { name = "my_extension", kind = "cdylib" },
# ]
```

#### `module-name`

By default maturin derives the extension module name from the Cargo package
name. Set `module-name` to override that, including with a dotted import path
such as `my_package._native`. That places the compiled extension inside a
Python package, which is the usual layout for mixed Python/Rust projects.

See [Import Rust as a submodule of your project](./project_layout.md#import-rust-as-a-submodule-of-your-project)
for a complete example (including matching `#[pymodule]` / `#[pyo3(name = ...)]`
changes).

#### `data`

Path to a wheel [data directory](https://packaging.python.org/en/latest/specifications/binary-distribution-format/#the-data-directory)
whose contents are installed into the corresponding wheel install schemes
(`data`, `scripts`, `headers`, `purelib`, `platlib`).

If unset, maturin uses `<module-name>.data` at the project root when that
directory exists. Relative paths are resolved from the project root.

See [Data](./project_layout.md#data) for the expected subdirectory layout.

#### `targets`

`targets` filters which Cargo compile targets maturin builds when a crate
exposes several candidates (for example multiple `cdylib` or `bin` targets).
It is **not** the same as `[tool.maturin.target.<triple>]`, which configures
per-architecture options such as the macOS deployment target.

```toml
[tool.maturin]
# Only build these Cargo targets (name must match Cargo.toml)
targets = [
  { name = "my_extension", kind = "cdylib" },
  { name = "my_cli", kind = "bin" },
]
```

`name` is required. `kind` is optional; when set it must match one of
`bin`, `cdylib`, `dylib`, `lib`, `rlib`, or `staticlib`.

#### `pgo-command`

Command used for the profile-training step of Profile-Guided Optimization.
Required when you pass `--pgo` (or set `MATURIN_PGO`). Maturin runs the command
in a temporary virtualenv after installing the instrumented wheel.

```toml
[tool.maturin]
pgo-command = "python -m pytest tests/benchmarks"
```

See the `--pgo` option under [Build](./distribution.md) for the overall
three-phase flow.

#### `generate-ci`

Defaults for `maturin generate-ci` (currently GitHub Actions). CLI flags that
overlap with this table are deprecated in favor of pyproject configuration.

```toml
[tool.maturin.generate-ci.github]
# Enable a pytest job in the generated workflow
pytest = true
# Use zig for manylinux cross compilation
zig = true
# Skip artifact attestation steps
skip-attestation = false
# Publish with PyPI trusted publishing (OIDC) instead of an API token
trusted-publishing = true
# Optional GitHub Actions environment name for the release job
publishing-environment = "release"
# Extra arguments passed to maturin on every platform
args = "--find-interpreter"

# Per-platform overrides. Each platform may set a simple `targets` list
# (architecture names such as "x86_64" / "aarch64") or a detailed
# `[[tool.maturin.generate-ci.github.<platform>.target]]` array, plus shared
# keys: runner, manylinux, container, docker-options, rust-toolchain,
# rustup-components, before-script-linux, args.
[tool.maturin.generate-ci.github.linux]
runner = "ubuntu-22.04"
manylinux = "2_28"
targets = ["x86_64", "aarch64"]

[tool.maturin.generate-ci.github.macos]
targets = ["aarch64"]

# Detailed per-target form (mutually exclusive with `targets` on the same platform):
# [[tool.maturin.generate-ci.github.linux.target]]
# arch = "x86_64"
# manylinux = "2_28"
#
# [[tool.maturin.generate-ci.github.linux.target]]
# arch = "aarch64"
# runner = "self-hosted-arm64"
# before-script-linux = "yum install -y openssl-devel"
```

Supported platform tables under `[tool.maturin.generate-ci.github]` are
`linux`, `musllinux`, `windows`, `macos`, `emscripten`, and `android`.

See [GitHub Actions](./distribution.md#github-actions) for usage of
`maturin generate-ci` and trusted publishing details.

The `[tool.maturin.include]` and `[tool.maturin.exclude]` configuration are
inspired by
[Poetry](https://python-poetry.org/docs/pyproject/#exclude-and-include).

Glob patterns are resolved relative to the directory containing `pyproject.toml`.
When using `python-source` (e.g. `python-source = "src/python"`), patterns are
also tried relative to the `python-source` directory if they don't match relative
to `pyproject.toml`. This means you can use a single pattern like
`include = ["mypackage/data.txt"]` and it will work for both sdist and wheel
targets: the sdist will include the file at its original location
(`src/python/mypackage/data.txt`), and the wheel will include it at the
package-relative path (`mypackage/data.txt`).

To specify files or globs directly:

```toml
include = ["path/**/*", "some/other/file"]
```

To specify a specific target format (`sdist` or `wheel`):

```toml
include = [
  { path = "path/**/*", format = "sdist" },
  { path = "all", format = ["sdist", "wheel"] },
  { path = "for/wheel/**/*", format = "wheel" }
]
```

The default behavior applies these configurations to both `sdist` and `wheel`
targets.

To include files generated by a Cargo build script (`build.rs`) from the
crate's `OUT_DIR`:

```toml
include = [
  { path = "cpu_features.json", from = "out-dir", to = "my_package/" },
]
```

The `path` is a glob pattern relative to `OUT_DIR`, and `to` specifies the
target directory inside the wheel. This only applies to wheel builds (not
sdist), since `OUT_DIR` does not exist until `cargo build` runs.

To include files from a different workspace member's `OUT_DIR`, specify the
crate name:

```toml
include = [
  { path = "generated/*.py", from = "out-dir", to = "my_package/", crate-name = "my-other-crate" },
]
```

#### SBOM options

```toml
[tool.maturin.sbom]
# Generate a CycloneDX SBOM for the Rust dependency tree.
# Defaults to true when the sbom feature is enabled.
rust = true
# Generate a CycloneDX SBOM for external shared libraries grafted during
# auditwheel repair. Defaults to true when repair copies libraries.
auditwheel = true
# Additional SBOM files to include in the wheel.
# Paths are relative to the project root.
include = ["sboms/vendor.cdx.json"]
```

See the [SBOM](./sbom.md) page for more details.

#### target specific maturin options

Currently only macOS deployment target SDK version can be configured
for `x86_64-apple-darwin` and `aarch64-apple-darwin` targets, other targets
have no options yet.

```toml
[tool.maturin.target.<triple>]
# macOS deployment target SDK version
macos-deployment-target = "11.0"
```
