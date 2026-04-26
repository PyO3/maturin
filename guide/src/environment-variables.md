# Environment Variables

Maturin reads a number of environment variables which you can use to configure the build process.
Here is a list of all environment variables that are read by maturin:

## Cargo environment variables

See [environment variables Cargo reads](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads)

## Python environment variables

* `VIRTUAL_ENV`: Path to a Python virtual environment
* `CONDA_PREFIX`: Path to a conda environment
* `_PYTHON_SYSCONFIGDATA_NAME`: Name of a `sysconfigdata*.py` file
* `MATURIN_PEP517_USE_BASE_PYTHON`: Use base Python executable instead of venv Python executable in PEP 517 build to avoid unnecessary rebuilds, should not be set when the sdist build requires packages installed in venv.

## PEP 517 build environment variables

* `MATURIN_PEP517_ARGS`: Extra arguments passed to `maturin` during PEP 517 builds (e.g. `pip install .`). The value is parsed using shell-style splitting. For example: `MATURIN_PEP517_ARGS="--features foo --profile release"`
* `MATURIN_NO_INSTALL_RUST`: If set, do not attempt to auto-install Rust via `puccinialin` when `cargo` is not found during PEP 517 builds.

### pip config-settings

You can also pass extra maturin arguments via pip's `--config-settings` flag:

```bash
pip install . --config-settings="build-args=--features foo"
# or with the namespaced key
pip install . --config-settings="maturin.build-args=--features foo"
```

Config-settings take priority over `MATURIN_PEP517_ARGS`; the environment variable is only used when no `build-args` config-setting is provided.

## Upload environment variables

* `MATURIN_PYPI_TOKEN`: PyPI token for uploading wheels (token-based authentication)
* `MATURIN_REPOSITORY`: The repository (package index) to upload the package to, defaults to `pypi`
* `MATURIN_REPOSITORY_URL`: The URL of the registry where the wheels are uploaded to, overrides `MATURIN_REPOSITORY`
* `MATURIN_USERNAME`: Username for pypi or your custom registry
* `MATURIN_PASSWORD`: Password for pypi or your custom registry
* `MATURIN_NON_INTERACTIVE`: Do not interactively prompt for username/password if the required credentials are missing

## `pyo3` environment variables

* `PYO3_CROSS_PYTHON_VERSION`: Python version to use for cross compilation
* `PYO3_CROSS_LIB_DIR`: This variable can be set to the directory containing the target's libpython DSO and the associated `_sysconfigdata*.py` file for Unix-like targets, or the Python DLL import libraries for the Windows target.
* `PYO3_CONFIG_FILE`: Path to a [pyo3 config file](https://pyo3.rs/latest/building-and-distribution.html#advanced-config-files)

## Networking environment variables

* `HTTP_PROXY` / `HTTPS_PROXY`: Proxy to use for HTTP/HTTPS requests
* `MATURIN_CA_BUNDLE` / `REQUESTS_CA_BUNDLE` / `CURL_CA_BUNDLE`: Path to a CA bundle to use for HTTPS requests

## Other environment variables

* `MACOSX_DEPLOYMENT_TARGET`: The minimum macOS version to target
* `IPHONEOS_DEPLOYMENT_TARGET`: The minimum iOS version to target
* `SOURCE_DATE_EPOCH`: The time to use for the timestamp in the wheel metadata
* `MATURIN_PYEMSCRIPTEN_PLATFORM_VERSION` / `PYEMSCRIPTEN_PLATFORM_VERSION`: The
  [PEP 783](https://peps.python.org/pep-0783/) PyEmscripten platform version
  (e.g. `2026_0`) used to derive the `pyemscripten_<year>_<patch>_wasm32`
  wheel platform tag. Pyodide 0.30+ exposes this via
  `sysconfig.get_config_var("PYEMSCRIPTEN_PLATFORM_VERSION")` and `pyodide
  config get pyemscripten_platform_version`.
* `MATURIN_PYODIDE_ABI_VERSION` / `PYODIDE_ABI_VERSION`: Pre-PEP 783 Pyodide
  ABI version (e.g. `2025_0`) used to derive the
  `pyodide_<year>_<patch>_wasm32` tag. Used when targeting Pyodide
  0.28 / 0.29. Available via `pyodide config get pyodide_abi_version`.
* `MATURIN_EMSCRIPTEN_VERSION`: The version of emscripten to use for emscripten
  builds (legacy `emscripten_<emcc-version>_wasm32` tag, used for Pyodide
  &le; 0.27 only). Prefer `MATURIN_PYEMSCRIPTEN_PLATFORM_VERSION` /
  `MATURIN_PYODIDE_ABI_VERSION` when possible — wheels built with the legacy
  tag are not installable on PEP 783-compliant runtimes.
* `MATURIN_STRIP`: Strip the library for minimum file size
* `MATURIN_NO_MISSING_BUILD_BACKEND_WARNING`: Suppress missing build backend warning
* `MATURIN_USE_XWIN`: Set to `1` to force to use `xwin` for cross compiling even on Windows that supports native compilation
* `ANDROID_API_LEVEL`: The Android API level to target when cross compiling for Android
* `TARGET_SYSROOT`: The sysroot to use for auditwheel wheel when cross compiling
* `ARCHFLAGS`: Flags to control the architecture of the build on macOS, for example you can use `ARCHFLAGS="-arch x86_64 -arch arm64"` to build universal2 wheels
