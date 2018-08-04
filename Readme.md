# Pyo3-pack

[![Linux and Mac Build Status](https://img.shields.io/travis/PyO3/pyo3-pack/master.svg?style=flat-square)](https://travis-ci.org/PyO3/pyo3-pack)
[![Windows Build status](https://ci.appveyor.com/api/projects/status/nns7qplb756sy4y7/branch/master?svg=true)](https://ci.appveyor.com/project/konstin/pyo3-pack/branch/master)
[![Crates.io](https://img.shields.io/crates/v/pyo3-pack.svg?style=flat-square)](https://crates.io/crates/pyo3-pack)
[![API Documentation on docs.rs](https://docs.rs/pyo3-pack/badge.svg)](https://docs.rs/pyo3-pack)
[![Chat on Gitter](https://img.shields.io/gitter/room/nwjs/nw.js.svg?style=flat-square)](https://gitter.im/PyO3/Lobby)

Build and publish crates with pyo3 bindings as python packages.

This project is meant as a zero configuration replacement for [setuptools-rust](https://github.com/PyO3/setuptools-rust). It supports building wheels for python 2.7 and 3.5+ on windows, linux and mac and can upload them to pypi.

## Usage

You can either download binaries from the [latest release](https://github.com/PyO3/pyo3-pack/releases/latest) or install it from source:

```shell
cargo install pyo3-pack
```

There are two subsommands: `publish` builds the crate into wheels and publishes them to pypi. The `build` subcommand builds the packages and stores them in a folder, but doesn't upload them. By default the target folder is `target/wheels`

The name of the package will be the name of the cargo project, i.e. the name field in the `[package]` section of Cargo.toml. The name of the module, which you are using when importing, will be the `name` value in the `[lib]` section (which defaults to the name of the package).

Pip allows adding so called console scripts, which are shell commands that execute some function in you program. You can add console scripts in a section `[package.metadata.pyo3-pack.scripts]`. The keys are the script names while the values are the path to the function in the format `some.module.path:class.function`, where the `class` part is optional. The function is called with no arguments. Example:

```toml
[package.metadata.pyo3-pack.scripts]
get_42 = "get_fourtytwo:DummyClass.get_42"
```

pyo3-pack can only build packages for installed python versions, so you might want to use e.g. pyenv, deadsnakes or docker for building.

### Build

```
USAGE:
    pyo3-pack build [FLAGS] [OPTIONS]

FLAGS:
    -d, --debug              Do a debug build (don't pass --release to cargo)
    -h, --help               Prints help information
        --skip-auditwheel    Don't check for manylinux compliance
        --use-cached         Don't rebuild if a wheel with the same name is already present
    -V, --version            Prints version information

OPTIONS:
    -b, --bindings-crate <binding_crate>    The crate providing the python bindings [default: pyo3]
    -i, --interpreter <interpreter>...      The python versions to build wheels for, given as the names of the
                                            interpreters. Uses a built-in list if not explicitly set.
    -m, --manifest-path <manifest_path>     The path to the Cargo.toml [default: .]
    -w, --wheel-dir <wheel_dir>             The directory to store the built wheels in. Defaults to a new "wheels"
                                            directory in the project's target directory
```

### Publish

```
USAGE:
    pyo3-pack publish [FLAGS] [OPTIONS]

FLAGS:
    -d, --debug              Do a debug build (don't pass --release to cargo)
    -h, --help               Prints help information
        --skip-auditwheel    Don't check for manylinux compliance
        --use-cached         Don't rebuild if a wheel with the same name is already present
    -V, --version            Prints version information

OPTIONS:
    -b, --bindings-crate <binding_crate>    The crate providing the python bindings [default: pyo3]
    -i, --interpreter <interpreter>...      The python versions to build wheels for, given as the names of the
                                            interpreters. Uses a built-in list if not explicitly set.
    -m, --manifest-path <manifest_path>     The path to the Cargo.toml [default: .]
    -p, --password <password>               Password for pypi or your custom registry
    -r, --repository-url <registry>         The url of registry where the wheels are uploaded to [default:
                                            https://upload.pypi.org/legacy/]
    -u, --username <username>               Username for pypi or your custom registry
    -w, --wheel-dir <wheel_dir>             The directory to store the built wheels in. Defaults to a new "wheels"
                                            directory in the project's target directory
```

## Code

The main part is the pyo3-pack library, which is completely documented and should be well integratable. The accompanying `main.rs` takes care username and password for the pypi upload and otherwise calls into the library. There is also a `get_fourtytwo` crate with python bindings and some dummy functionally (such as returning 42) and and the integration test folder testing with pyo3-pack with get_fourtytwo. The `sysconfig` folder contains the output of `python -m sysconfig` for different python versions and platform, which is helpful during development.

You might want to have look into my [blog post](https://blog.schuetze.link/2018/07/21/a-dive-into-packaging-native-python-extensions.html) which explains all the nitty-gritty details on building python packages.
