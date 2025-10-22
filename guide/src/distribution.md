# Distribution

## Build Wheels

When building Rust binary or library, it's possible to depend on libraries and symbols only available on the build machine.
To ensure the wheels are portable, native python modules on linux must only dynamically link a set of libraries and symbols called manylinux and musllinux.
The pypa offers special docker images to ensure compliance with the [manylinux rules](https://peps.python.org/pep-0599/#the-manylinux2014-policy).
If you want to publish linux wheels on pypi, **you need to use a manylinux docker image or [build with zig](#use-zig)**.

The Rust compiler since version 1.64 [requires at least glibc 2.17](https://blog.rust-lang.org/2022/08/01/Increasing-glibc-kernel-requirements.html), so you need to use at least manylinux2014.
For publishing, we recommend enforcing the same manylinux version as the image with the manylinux flag, e.g. use `--manylinux 2014` if you are building in `quay.io/pypa/manylinux2014_x86_64`.
The [PyO3/maturin-action](https://github.com/PyO3/maturin-action) github action already takes care of this if you set e.g. `manylinux: 2014`.

If you are publishing to PyPI, you can use `--compatibility pypi` to allow only builds for targets that are accepted by PyPI, and reject builds for unsupported operating systems and architectures.

maturin can check the generated library for manylinux compliance (an auditwheel reimplementation) and gives the wheel the proper platform tag.

- If your system's glibc is too new, it will assign the `linux` tag.
- If you link other shared libraries, maturin will try to bundle them within the wheel, note that this requires [patchelf](https://github.com/NixOS/patchelf),
  which can be installed along with maturin from PyPI: `pip install maturin[patchelf]`.

You can also manually disable those checks and directly use native linux target with `--manylinux off`.

The [pyo3/maturin](https://ghcr.io/pyo3/maturin) image is based on the manylinux2014 image, and passes arguments to the `maturin` binary. You can use it like this:

```
docker run --rm -v $(pwd):/io ghcr.io/pyo3/maturin build --release  # or other maturin arguments
```

Note that this image is very basic and only contains python, maturin and stable Rust. If you need additional tools, you can run commands inside the manylinux container.
See [konstin/complex-manylinux-maturin-docker](https://github.com/konstin/complex-manylinux-maturin-docker) for a small educational example
or [nanoporetech/fast-ctc-decode](https://github.com/nanoporetech/fast-ctc-decode/blob/b226ea0f2b2f4f474eff47349703d57d2ea4801b/.github/workflows/publish.yml) for a real world setup.

```
Usage: maturin build [OPTIONS] [ARGS]...

Arguments:
  [ARGS]...
          Rustc flags

Options:
      --strip
          Strip the library for minimum file size

      --sdist
          Build a source distribution

      --compatibility [<compatibility>...]
          Control platform tags. Use `pypi` to ensure PyPI compatibility, or specify platform-specific
          tags like `manylinux2014`, `musllinux_1_2`, or `linux`.

          The default is the lowest compatible `manylinux` tag, or plain `linux` if nothing matched

  -i, --interpreter [<INTERPRETER>...]
          The python versions to build wheels for, given as the executables of interpreters such as `python3.9` or `/usr/bin/python3.8`

  -f, --find-interpreter
          Find interpreters from the host machine

  -b, --bindings <BINDINGS>
          Which kind of bindings to use

          [possible values: pyo3, pyo3-ffi, cffi, uniffi, bin]

  -o, --out <OUT>
          The directory to store the built wheels in. Defaults to a new "wheels" directory in the project's target directory

      --auditwheel <AUDITWHEEL>
          Audit wheel for manylinux compliance

          Possible values:
          - repair: Audit and repair wheel for manylinux compliance
          - check:  Check wheel for manylinux compliance, but do not repair
          - skip:   Don't check for manylinux compliance

      --zig
          For manylinux targets, use zig to ensure compliance for the chosen manylinux version

          Default to manylinux2014/manylinux_2_17 if you do not specify an `--compatibility`

          Make sure you installed zig with `pip install maturin[zig]`

  -q, --quiet
          Do not print cargo log messages

      --ignore-rust-version
          Ignore `rust-version` specification in packages

  -v, --verbose...
          Use verbose output (-vv very verbose/build.rs output)

      --color <WHEN>
          Coloring: auto, always, never

      --config <KEY=VALUE>
          Override a configuration value (unstable)

  -Z <FLAG>
          Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details

      --future-incompat-report
          Outputs a future incompatibility report at the end of the build (unstable)

      --compression-method <COMPRESSION_METHOD>
          Zip compresson method to use

          [default: deflated]

          Possible values:
          - deflated: Deflate compression
          - stored:   No compression
          - zstd:     Zstandard compression

  -h, --help
          Print help (see a summary with '-h')

Compilation Options:
  -r, --release
          Build artifacts in release mode, with optimizations

  -j, --jobs <N>
          Number of parallel jobs, defaults to # of CPUs

      --profile <PROFILE-NAME>
          Build artifacts with the specified Cargo profile

      --target <TRIPLE>
          Build for the target triple

          [env: CARGO_BUILD_TARGET=]

      --target-dir <DIRECTORY>
          Directory for all generated artifacts

      --timings=<FMTS>
          Timing output formats (unstable) (comma separated): html, json

Feature Selection:
  -F, --features <FEATURES>
          Space or comma separated list of features to activate

      --all-features
          Activate all available features

      --no-default-features
          Do not activate the `default` feature

Manifest Options:
  -m, --manifest-path <PATH>
          Path to Cargo.toml

      --frozen
          Require Cargo.lock and cache are up to date

      --locked
          Require Cargo.lock is up to date

      --offline
          Run without accessing the network
```

## Source Distribution

Maturin supports building through `pyproject.toml`. To use it, create a `pyproject.toml` next to your `Cargo.toml` with the following content:

```toml
[build-system]
requires = ["maturin>=1.0,<2.0"]
build-backend = "maturin"
```

If a `pyproject.toml` with a `[build-system]` entry is present, maturin can build a source distribution of your package when `--sdist` is specified.
The source distribution will contain the same files as `cargo package`. To only build a source distribution, use the `maturin sdist` command.

You can then e.g. install your package with `pip install .`. With `pip install . -v` you can see the output of cargo and maturin.

You can use the options `compatibility`, `skip-auditwheel`, `bindings`, `strip` and common Cargo build options such as `features` under `[tool.maturin]` the same way you would when running maturin directly.
The `bindings` key is required for cffi and bin projects as those can't be automatically detected. Currently, all builds are in release mode (see [this thread](https://discuss.python.org/t/pep-517-debug-vs-release-builds/1924) for details).

For a non-manylinux build with cffi bindings you could use the following:

```toml
[build-system]
requires = ["maturin>=1.0,<2.0"]
build-backend = "maturin"

[tool.maturin]
bindings = "cffi"
compatibility = "linux"
```

`manylinux` option is also accepted as an alias of `compatibility` for backward compatibility with old version of maturin.

To include arbitrary files in the sdist for use during compilation specify `include` as an array of `path` globs with `format` set to `sdist`:

```toml
[tool.maturin]
include = [{ path = "path/**/*", format = "sdist" }]
```

### Cross Compiling

Maturin has decent cross compilation support for `pyo3` and `bin` bindings,
other kind of bindings may work but aren't tested regularly.

#### Cross-compile to Linux/macOS

##### Use Docker

For manylinux support the [manylinux-cross](https://github.com/rust-cross/manylinux-cross) docker images can be used.
And [maturin-action](https://github.com/PyO3/maturin-action) makes it easy to do cross compilation on GitHub Actions.

##### Use Zig

Since v0.12.7 maturin added support for linking with [`zig cc`](https://andrewkelley.me/post/zig-cc-powerful-drop-in-replacement-gcc-clang.html),
compile for Linux works and is regularly tested on CI, other platforms may also work but aren't tested regularly.

You can install zig following the [official documentation](https://ziglang.org/download), or install it from PyPI via `pip install ziglang`.
Then pass `--zig` to maturin `build` or `publish` commands to use it, for example

```bash
maturin build --release --target aarch64-unknown-linux-gnu --zig
```

#### Cross-compile to Windows

Pyo3 0.16.5 added an experimental feature `generate-import-lib` enables the user to cross compile
extension modules for Windows targets without setting the `PYO3_CROSS_LIB_DIR` environment variable
or providing any Windows Python library files.

```toml
[dependencies]
pyo3 = { version = "0.27.0", features = ["extension-module", "generate-import-lib"] }
```

It uses an external [`python3-dll-a`](https://docs.rs/python3-dll-a/latest/python3_dll_a/) crate to
generate import libraries for the Python DLL for MinGW-w64 and MSVC compile targets.
Note: MSVC targets require LLVM binutils or MSVC build tools to be available on the host system.
More specifically, `python3-dll-a` requires `llvm-dlltool` or `lib.exe` executable to be present in `PATH` when targeting `*-pc-windows-msvc`.

maturin integrates [`cargo-xwin`](https://github.com/messense/cargo-xwin) to enable MSVC targets cross compilation support,
it will download and unpack the Microsoft CRT headers and import libraries, and Windows SDK headers and import libraries
needed for compiling and linking automatically.

**By using this to cross compiling to Windows MSVC targets you are consented to accept the license at [https://go.microsoft.com/fwlink/?LinkId=2086102](https://go.microsoft.com/fwlink/?LinkId=2086102)**.
(Building on Windows natively does not apply.)

## GitHub Actions

If your project uses GitHub Actions, you can use the `maturin generate-ci` command to generate a GitHub Actions workflow file.

```bash
mkdir -p .github/workflows
maturin generate-ci github > .github/workflows/CI.yml
```

There are some options to customize the generated workflow file:

```
Generate CI configuration

Usage: maturin generate-ci [OPTIONS] <CI>

Arguments:
  <CI>
          CI provider

          Possible values:
          - github: GitHub

Options:
  -m, --manifest-path <PATH>
          Path to Cargo.toml

  -v, --verbose...
          Use verbose output.

          * Default: Show build information and `cargo build` output. * `-v`: Use `cargo build -v`.
          * `-vv`: Show debug logging and use `cargo build -vv`. * `-vvv`: Show trace logging.

          You can configure fine-grained logging using the `RUST_LOG` environment variable.
          (<https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives>)

  -o, --output <PATH>
          Output path

          [default: -]

      --platform <platform>...
          Platform support

          [default: linux musllinux windows macos]

          Possible values:
          - all:        All
          - manylinux:  Manylinux
          - musllinux:  Musllinux
          - windows:    Windows
          - macos:      macOS
          - emscripten: Emscripten

      --pytest
          Enable pytest

      --zig
          Use zig to do cross compilation

      --skip-attestation
          Skip artifact attestation

  -h, --help
          Print help (see a summary with '-h')
```

### Using PyPI's trusted publishing

By default, the workflow provided by `generate-ci` will publish the release artifacts to PyPI using API token authentication. However, maturin also supports [trusted publishing (OpenID Connect)](https://docs.pypi.org/trusted-publishers/).

To enable it, modify the `release` action in the generated GitHub workflow file:

- remove `MATURIN_PYPI_TOKEN` from the `env` section to make maturin use trusted publishing
- add `id-token: write` to the action's `permissions` (see [Configuring OpenID Connect in PyPI](https://docs.github.com/en/actions/deployment/security-hardening-your-deployments/configuring-openid-connect-in-pypi) from GitHub's documentation).
- if `Environment name: pypi` was set in PyPI, add `environment: pypi`

Make sure to follow the steps listed in [PyPI's documentation](https://docs.pypi.org/trusted-publishers/adding-a-publisher/) to set up your GitHub repository as a trusted publisher in the PyPI project settings before attempting to run the workflow.
