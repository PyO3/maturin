# maturin is self bootstraping, however on platforms like alpine linux that aren't
# manylinux, pip will try installing maturin from the source distribution.
# That source distribution obviously can't depend on maturin, so we're using
# the always available setuptools.
#
# Note that this is really only a workaround for bootstrapping and not suited
# for general purpose packaging, i.e. only building a wheel (as in
# `python setup.py bdist_wheel`) and installing (as in
# `pip install <source dir>` are supported. For creating a source distribution
# for maturin itself use `maturin sdist`.

import platform
import sys

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
# but that would requires python-dev
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

with open("Readme.md", encoding="utf-8", errors="ignore") as fp:
    long_description = fp.read()

with open("Cargo.toml", "rb") as fp:
    version = tomllib.load(fp)["package"]["version"]

cargo_args = []
if platform.machine() in (
    "mips",
    "mips64",
    "ppc",
    "ppc64le",
    "ppc64",
    "powerpc",
    "riscv64",
    "sparc64",
) or (sys.platform == "win32" and platform.machine() == "ARM64"):
    cargo_args.extend(["--no-default-features", "--features=upload,log,human-panic"])
elif sys.platform.startswith("haiku"):
    # mio and ring doesn't build on haiku
    cargo_args.extend(["--no-default-features", "--features=log,human-panic"])

setup(
    name="maturin",
    author="konstin",
    author_email="konstin@mailbox.org",
    url="https://github.com/pyo3/maturin",
    description="Build and publish crates with pyo3, rust-cpython and cffi bindings as well as rust binaries as "
    "python packages",
    long_description=long_description,
    long_description_content_type="text/markdown",
    version=version,
    license="MIT OR Apache-2.0",
    python_requires=">=3.7",
    cmdclass={"bdist_wheel": bdist_wheel},
    packages=["maturin"],
    rust_extensions=[RustBin("maturin", args=cargo_args)],
    classifiers=[
        "Topic :: Software Development :: Build Tools",
        "Programming Language :: Rust",
        "Programming Language :: Python :: Implementation :: CPython",
        "Programming Language :: Python :: Implementation :: PyPy",
    ],
    install_requires=["tomli>=1.1.0 ; python_version<'3.11'"],
    setup_requires=["setuptools-rust>=1.4.0"],
    zip_safe=False,
)
