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

There are three main subsommands:

 * `publish` builds the crate into python packages and publishes them to pypi.
 * `build` builds the wheels and stores them in a folder (`target/wheels` by default), but doesn't upload them.
 * `develop` builds the crate and install it's as a python module directly in the current virtualenv

pyo3-pack runs directly on a crate, with no extra files needed, and also doesn't clash with an existing setuptools-rust configuration. You can even integrate it with testing tools such as tox (see `get-fourtytwo` for an example).

The name of the package will be the name of the cargo project, i.e. the name field in the `[package]` section of Cargo.toml. The name of the module, which you are using when importing, will be the `name` value in the `[lib]` section (which defaults to the name of the package).

Pip allows adding so called console scripts, which are shell commands that execute some function in you program. You can add console scripts in a section `[package.metadata.pyo3-pack.scripts]`. The keys are the script names while the values are the path to the function in the format `some.module.path:class.function`, where the `class` part is optional. The function is called with no arguments. Example:

```toml
[package.metadata.pyo3-pack.scripts]
get_42 = "get_fourtytwo:DummyClass.get_42"
```

pyo3-pack can only build packages for installed python versions, so you might want to use e.g. pyenv, deadsnakes or docker for building. If you don't set your own interpreters with `-i`, a heuristic is used to search for python installations. You can get a list of those with the `list-python` subcommand.

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

### Develop

```
USAGE:
    pyo3-pack develop [OPTIONS]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -b, --bindings-crate <binding_crate>            The crate providing the python bindings [default: pyo3]
        --cargo-extra-args <cargo_extra_args>...
            Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`

    -m, --manifest-path <manifest_path>             The path to the Cargo.toml [default: Cargo.toml]
        --rustc-extra-args <rustc_extra_args>...
            Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
```

### Manylinux and auditwheel

For portability reasons, native python modules on linux must only dynamically link a set of very few libraries which are installed basically everywhere, hence the name manylinux. The pypa offers a special docker container and a tool called [auditwheel](https://github.com/pypa/auditwheel/) to ensure compliance with the [manylinux rules](https://www.python.org/dev/peps/pep-0513/#the-manylinux1-policy). pyo3-pack contains a reimplementation of the most important part of auditwheel that checks the generated library, so there's no need to use external tools. If you want to disable the manylinux compliance checks for some reason, use the `--skip-auditwheel` flag.

## Code

The main part is the pyo3-pack library, which is completely documented and should be well integratable. The accompanying `main.rs` takes care username and password for the pypi upload and otherwise calls into the library. There is also a `get_fourtytwo` crate with python bindings and some dummy functionally (such as returning 42) and and the integration test folder testing with pyo3-pack with get_fourtytwo. The `sysconfig` folder contains the output of `python -m sysconfig` for different python versions and platform, which is helpful during development.

You need to install `virtualenv` (`pip install virtualenv`) to run the tests.

You might want to have look into my [blog post](https://blog.schuetze.link/2018/07/21/a-dive-into-packaging-native-python-extensions.html) which explains the intricacies of building native python packages.
