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

You can use the options `compatibility`, `skip-auditwheel`, `bindings`, `strip`, `cargo-extra-args` and `rustc-extra-args` under `[tool.maturin]` the same way you would when running maturin directly.
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

To include arbitrary files in the sdist for use during compilation specify `sdist-include` as an array of globs:

```toml
[tool.maturin]
sdist-include = ["path/**/*"]
```

## Build Wheels

For portability reasons, native python modules on linux must only dynamically link a set of very few libraries which are installed basically everywhere, hence the name manylinux.
The pypa offers special docker images and a tool called [auditwheel](https://github.com/pypa/auditwheel/) to ensure compliance with the [manylinux rules](https://www.python.org/dev/peps/pep-0571/#the-manylinux2010-policy).
If you want to publish widely usable wheels for linux pypi, **you need to use a manylinux docker image** or [build with zig](#use-zig).

The Rust compiler since version 1.47 [requires at least glibc 2.11](https://github.com/rust-lang/rust/blob/master/RELEASES.md#version-1470-2020-10-08), so you need to use at least manylinux2010.
For publishing, we recommend enforcing the same manylinux version as the image with the manylinux flag, e.g. use `--manylinux 2014` if you are building in `quay.io/pypa/manylinux2014_x86_64`.
The [messense/maturin-action](https://github.com/messense/maturin-action) github action already takes care of this if you set e.g. `manylinux: 2014`.

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
    maturin build [OPTIONS]

OPTIONS:
    -b, --bindings <BINDINGS>
            Which kind of bindings to use. Possible values are pyo3, rust-cpython, cffi and bin

        --cargo-extra-args <CARGO_EXTRA_ARGS>
            Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --
            [...]`

            Use as `--cargo-extra-args="--my-arg"`

            Note that maturin invokes cargo twice: Once as `cargo metadata` and then as `cargo
            rustc`. maturin tries to pass only the shared subset of options to cargo metadata, but
            this is may be a bit flaky.

        --compatibility <compatibility>
            Control the platform tag on linux.

            Options are `manylinux` tags (for example `manylinux2014`/`manylinux_2_24`) or
            `musllinux` tags (for example `musllinux_1_2`) and `linux` for the native linux tag.

            Note that `manylinux1` is unsupported by the rust compiler. Wheels with the native
            `linux` tag will be rejected by pypi, unless they are separately validated by
            `auditwheel`.

            The default is the lowest compatible `manylinux` tag, or plain `linux` if nothing
            matched

            This option is ignored on all non-linux platforms

    -h, --help
            Print help information

    -i, --interpreter <INTERPRETER>
            The python versions to build wheels for, given as the names of the interpreters. Uses
            autodiscovery if not explicitly set

    -m, --manifest-path <PATH>
            The path to the Cargo.toml

        --sdist
            Build a source distribution

    -o, --out <OUT>
            The directory to store the built wheels in. Defaults to a new "wheels" directory in the
            project's target directory

        --release
            Pass --release to cargo

        --rustc-extra-args <RUSTC_EXTRA_ARGS>
            Extra arguments that will be passed to rustc as `cargo rustc [...] -- [...] [arg1]
            [arg2]`

            Use as `--rustc-extra-args="--my-arg"`

        --skip-auditwheel
            Don't check for manylinux compliance

        --strip
            Strip the library for minimum file size

        --target <TRIPLE>
            The --target option for cargo

            [env: CARGO_BUILD_TARGET=]

        --universal2
            Control whether to build universal2 wheel for macOS or not. Only applies to macOS
            targets, do nothing otherwise

        --zig
            For manylinux targets, use zig to ensure compliance for the chosen manylinux version

            Default to manylinux2010/manylinux_2_12 if you do not specify an `--compatibility`

            Make sure you installed zig with `pip install maturin[zig]`
```

### Cross Compiling

Maturin has decent cross compilation support for `pyo3` and `bin` bindings,
other kind of bindings may work but aren't tested regularly.

#### Use Docker

For manylinux support the [manylinux-cross](https://github.com/messense/manylinux-cross) docker images can be used.
And [maturin-action](https://github.com/messense/maturin-action) makes it easy to do cross compilation on GitHub Actions.

#### Use Zig

Since v0.12.7 maturin added support for linking with [`zig cc`](https://andrewkelley.me/post/zig-cc-powerful-drop-in-replacement-gcc-clang.html),
compile for  Linux works and is regularly tested on CI, other platforms may also work but aren't tested regularly.

You can install zig following the [official documentation](https://ziglang.org/download), or install it from PyPI via `pip install ziglang`.
Then pass `--zig` to maturin `build` or `publish` commands to use it, for example

```bash
maturin build --release --target aarch64-unknown-linux-gnu --zig
```
