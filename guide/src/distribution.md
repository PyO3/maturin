# Distribution

## Source Distribution

Maturin supports building through `pyproject.toml`. To use it, create a `pyproject.toml` next to your `Cargo.toml` with the following content:

```toml
[build-system]
requires = ["maturin>=0.13,<0.14"]
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
requires = ["maturin>=0.13,<0.14"]
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

## Build Wheels

For portability reasons, native python modules on linux must only dynamically link a set of very few libraries which are installed basically everywhere, hence the name manylinux.
The pypa offers special docker images and a tool called [auditwheel](https://github.com/pypa/auditwheel/) to ensure compliance with the [manylinux rules](https://peps.python.org/pep-0599/#the-manylinux2014-policy)).
If you want to publish widely usable wheels for linux pypi, **you need to use a manylinux docker image** or [build with zig](#use-zig).

The Rust compiler since version 1.64 [requires at least glibc 2.17](https://blog.rust-lang.org/2022/08/01/Increasing-glibc-kernel-requirements.html), so you need to use at least manylinux2014.
For publishing, we recommend enforcing the same manylinux version as the image with the manylinux flag, e.g. use `--manylinux 2014` if you are building in `quay.io/pypa/manylinux2014_x86_64`.
The [PyO3/maturin-action](https://github.com/PyO3/maturin-action) github action already takes care of this if you set e.g. `manylinux: 2014`.

maturin contains a reimplementation of auditwheel automatically checks the generated library and gives the wheel the proper platform tag.

* If your system's glibc is too new, it will assign the `linux` tag.
* If you link other shared libraries, maturin will try to bundle them within the wheel, note that this requires [patchelf](https://github.com/NixOS/patchelf), 
  it can be installed along with maturin from PyPI: `pip install maturin[patchelf]`.

You can also manually disable those checks and directly use native linux target with `--manylinux off`.

For full manylinux compliance you need to compile in a CentOS docker container. The [pyo3/maturin](https://ghcr.io/pyo3/maturin) image is based on the manylinux2010 image,
and passes arguments to the `maturin` binary. You can use it like this:

```
docker run --rm -v $(pwd):/io ghcr.io/pyo3/maturin build --release  # or other maturin arguments
```

Note that this image is very basic and only contains python, maturin and stable Rust. If you need additional tools, you can run commands inside the manylinux container.
See [konstin/complex-manylinux-maturin-docker](https://github.com/konstin/complex-manylinux-maturin-docker) for a small educational example 
or [nanoporetech/fast-ctc-decode](https://github.com/nanoporetech/fast-ctc-decode/blob/b226ea0f2b2f4f474eff47349703d57d2ea4801b/.github/workflows/publish.yml) for a real world setup.


```
USAGE:
    maturin build [OPTIONS] [--] [ARGS]...

ARGS:
    <ARGS>...
            Rustc flags

OPTIONS:
    -r, --release
            Build artifacts in release mode, with optimizations

        --strip
            Strip the library for minimum file size

        --sdist
            Build a source distribution

        --compatibility <compatibility>...
            Control the platform tag on linux.

            Options are `manylinux` tags (for example `manylinux2014`/`manylinux_2_24`) or
            `musllinux` tags (for example `musllinux_1_2`) and `linux` for the native linux tag.

            Note that `manylinux1` and `manylinux2010` is unsupported by the rust compiler. Wheels
            with the native `linux` tag will be rejected by pypi, unless they are separately
            validated by `auditwheel`.

            The default is the lowest compatible `manylinux` tag, or plain `linux` if nothing
            matched

            This option is ignored on all non-linux platforms

    -i, --interpreter <INTERPRETER>...
            The python versions to build wheels for, given as the names of the interpreters

    -f, --find-interpreter
            Find interpreters from the host machine

    -b, --bindings <BINDINGS>
            Which kind of bindings to use. Possible values are pyo3, rust-cpython, cffi and bin

    -o, --out <OUT>
            The directory to store the built wheels in. Defaults to a new "wheels" directory in the
            project's target directory

        --skip-auditwheel
            Don't check for manylinux compliance

        --zig
            For manylinux targets, use zig to ensure compliance for the chosen manylinux version

            Default to manylinux2014/manylinux_2_17 if you do not specify an `--compatibility`

            Make sure you installed zig with `pip install maturin[zig]`

        --universal2
            Control whether to build universal2 wheel for macOS or not. Only applies to macOS
            targets, do nothing otherwise

    -q, --quiet
            Do not print cargo log messages

    -j, --jobs <N>
            Number of parallel jobs, defaults to # of CPUs

        --profile <PROFILE-NAME>
            Build artifacts with the specified Cargo profile

    -F, --features <FEATURES>
            Space or comma separated list of features to activate

        --all-features
            Activate all available features

        --no-default-features
            Do not activate the `default` feature

        --target <TRIPLE>
            Build for the target triple

            [env: CARGO_BUILD_TARGET=]

        --target-dir <DIRECTORY>
            Directory for all generated artifacts

    -m, --manifest-path <PATH>
            Path to Cargo.toml

        --ignore-rust-version
            Ignore `rust-version` specification in packages

    -v, --verbose
            Use verbose output (-vv very verbose/build.rs output)

        --color <WHEN>
            Coloring: auto, always, never

        --frozen
            Require Cargo.lock and cache are up to date

        --locked
            Require Cargo.lock is up to date

        --offline
            Run without accessing the network

        --config <KEY=VALUE>
            Override a configuration value (unstable)

    -Z <FLAG>
            Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details

        --timings[=<FMTS>...]
            Timing output formats (unstable) (comma separated): html, json

        --future-incompat-report
            Outputs a future incompatibility report at the end of the build (unstable)

    -h, --help
            Print help information
```

### Cross Compiling

Maturin has decent cross compilation support for `pyo3` and `bin` bindings,
other kind of bindings may work but aren't tested regularly.

#### Cross-compile to Linux/macOS

##### Use Docker

For manylinux support the [manylinux-cross](https://github.com/messense/manylinux-cross) docker images can be used.
And [maturin-action](https://github.com/PyO3/maturin-action) makes it easy to do cross compilation on GitHub Actions.

##### Use Zig

Since v0.12.7 maturin added support for linking with [`zig cc`](https://andrewkelley.me/post/zig-cc-powerful-drop-in-replacement-gcc-clang.html),
compile for  Linux works and is regularly tested on CI, other platforms may also work but aren't tested regularly.

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
pyo3 = { version = "0.17.3", features = ["extension-module", "generate-import-lib"] }
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
