# Pyo3-pack

Build and publish crates with pyo3 bindings as python packages.

### Usage

You can install pyo3-pack with

```shell
cargo install pyo3-pack
```

The `publish` subcommand builds the crate into python packages and publishes the wheels to pypi. The `build` subcommand builds the packages and stores them in a folder, but doesn't upload them.

The name of the package will be the name field of the `[lib]` section in the Cargot.toml, which defaults to the name of the package.

You can add console scripts in a section `[package.metadata.pyo3-pack.scripts]`. The keys are the script names while the values are the path to the function in the format `some.module.path:class.function`, where the `class` part is optional. Example:

```toml
[package.metadata.pyo3-pack.scripts]
get_42 = "get_fourtytwo:DummyClass.get_42"
```

pyo3-pack can only build packages for installed python versions, so you might need to use e.g. deadsnakes or docker for building.

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
    -m, --manifest-path <manifest_path>     The path to the Cargo.toml or the directory containing it [default: .]
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
    -m, --manifest-path <manifest_path>     The path to the Cargo.toml or the directory containing it [default: .]
    -p, --password <password>               Password for pypi or your custom registry
    -r, --repository-url <registry>         The url of registry where the wheels are uploaded to [default:
                                            https://upload.pypi.org/legacy/]
    -u, --username <username>               Username for pypi or your custom registry
    -w, --wheel-dir <wheel_dir>             The directory to store the built wheels in. Defaults to a new "wheels"
                                            directory in the project's target directory
```

## Code

This repository consists of the main pyo3-pack crate, which is a library with a single binary target that is mostly handling username and password for the pypi upload, a `get_fourtytwo` crate with python bindings and some dummy functionally (such as returning 42) and the integration test folder with some basic testing utilities.

You might want to have look into my [blog post](1https://blog.schuetze.link/python/rust/2018/07/21/a-dive-into-packaging-native-python-extensions.html) which explains all the nitty-gritty details on building python packages.
