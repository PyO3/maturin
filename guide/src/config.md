# Configuration

## Configuration format

You can configure maturin in `tool.maturin` section of [`pyproject.toml`](https://peps.python.org/pep-0518/#tool-table).

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
# Don't check for manylinux compliance, deprecated in favor of auditwheel = "audit"
skip-auditwheel = false
# Python source directory
python-source = "src"
# Python packages to include
python-packages = ["foo", "bar"]
# Strip the library for minimum file size
strip = true
# Source distribution generator,
# supports cargo (default) and git.
sdist-generator = "cargo"
# Use base Python executable instead of venv Python executable in PEP 517 build.
#
# This can help avoid unnecessary rebuilds, as the Python executable does not change
# every time. It should not be set when the sdist build requires packages installed
# in venv. This can also be set with the `MATURIN_PEP517_USE_BASE_PYTHON` environment
# variable.
use-base-python = false
```

The `[tool.maturin.include]` and `[tool.maturin.exclude]` configuration are
inspired by
[Poetry](https://python-poetry.org/docs/pyproject/#include-and-exclude).

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

The default behavior is apply these configurations to both `sdist` and `wheel`
targets.

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
