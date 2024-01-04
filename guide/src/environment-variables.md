# Environment Variables

Maturin reads a number of environment variables which you can use to configure the build process.
Here is a list of all environment variables that are read by maturin:

## Cargo environment variables
See [environment variables Cargo reads](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads)

## Python environment variables

* `VIRTUAL_ENV`: Path to a Python virtual environment
* `CONDA_PREFIX`: Path to a conda environment
* `MATURIN_PYTHON_SYSCONFIGDATA_DIR`: Path to a directory containing a `sysconfigdata*.py` file
* `_PYTHON_SYSCONFIGDATA_NAME`: Name of a `sysconfigdata*.py` file
* `MATURIN_PYPI_TOKEN`: PyPI token for uploading wheels
* `MATURIN_PASSWORD`: PyPI password for uploading wheels

## Import hook environment variables

* `MATURIN_BUILD_DIR`: Path to a location to cache build files
* `MATURIN_IMPORT_HOOK_ENABLED`: set to `0` to disable calls to `import_hook.install()`

## `pyo3` environment variables

* `PYO3_CROSS_PYTHON_VERSION`: Python version to use for cross compilation
* `PYO3_CROSS_LIB_DIR`: This variable can be set to the directory containing the target's libpython DSO and the associated `_sysconfigdata*.py` file for Unix-like targets, or the Python DLL import libraries for the Windows target.This variable can be set to the directory containing the target's libpython DSO and the associated _sysconfigdata*.py file for Unix-like targets, or the Python DLL import libraries for the Windows target.
* `PYO3_CONFIG_FILE`: Path to a [pyo3 config file](https://pyo3.rs/latest/building_and_distribution.html#advanced-config-files)

## Networking environment variables

* `HTTP_PROXY` / `HTTPS_PROXY`: Proxy to use for HTTP/HTTPS requests
* `REQUESTS_CA_BUNDLE` / `CURL_CA_BUNDLE`: Path to a CA bundle to use for HTTPS requests

## Other environment variables

* `MACOSX_DEPLOYMENT_TARGET`: The minimum macOS version to target
* `SOURCE_DATE_EPOCH`: The time to use for the timestamp in the wheel metadata
* `MATURIN_EMSCRIPTEN_VERSION`: The version of emscripten to use for emscripten builds
* `TARGET_SYSROOT`: The sysroot to use for auditwheel wheel when cross compiling
* `ARCHFLAGS`: Flags to control the architecture of the build on macOS, for example you can use `ARCHFLAGS="-arch x86_64 -arch arm64"` to build universal2 wheels
