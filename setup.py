# maturin is self bootstrapping, however on platforms like FreeBSD that aren't
# manylinux/musllinux, pip will try installing maturin from the source distribution.
# That source distribution obviously can't depend on maturin, so we're using
# the always available setuptools.
#
# Note that this is really only a workaround for bootstrapping and not suited
# for general purpose packaging, i.e. only building a wheel (as in
# `python setup.py bdist_wheel`) and installing (as in
# `pip install <source dir>`) are supported. For creating a source distribution
# for maturin itself use `maturin sdist`.

import os
import shlex
import shutil

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib
from setuptools import setup

from setuptools_rust import RustBin

# Force the wheel to be platform specific
# https://stackoverflow.com/a/45150383/3549270
# There's also the much more concise solution in
# https://stackoverflow.com/a/53463910/3549270,
# but that would require python-dev
try:
    # noinspection PyPackageRequirements,PyUnresolvedReferences
    from wheel.bdist_wheel import bdist_wheel as _bdist_wheel

    # noinspection PyPep8Naming,PyAttributeOutsideInit
    class bdist_wheel(_bdist_wheel):
        def finalize_options(self):
            _bdist_wheel.finalize_options(self)
            self.root_is_pure = False

except ImportError:
    bdist_wheel = None

with open("Cargo.toml", "rb") as fp:
    version = tomllib.load(fp)["package"]["version"]

# Use `--no-default-features` by default for a minimal build to support PEP 517.
# `MATURIN_SETUP_ARGS` env var can be used to pass customized arguments to cargo.
cargo_args = ["--no-default-features"]
if os.getenv("MATURIN_SETUP_ARGS"):
    cargo_args = shlex.split(os.getenv("MATURIN_SETUP_ARGS", ""))

if not os.environ.get("MATURIN_NO_INSTALL_RUST") and not shutil.which("cargo"):
    from puccinialin import setup_rust

    print("Rust not found, installing into a temporary directory")
    extra_env = setup_rust()
    env = {**os.environ, **extra_env}
else:
    env = None

setup(
    version=version,
    cmdclass={"bdist_wheel": bdist_wheel},
    rust_extensions=[RustBin("maturin", args=cargo_args, cargo_manifest_args=["--locked"], env=env)],
    zip_safe=False,
)
