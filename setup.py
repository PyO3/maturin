# pyo3 is self bootstraping, however on platforms like alpine linux that aren't
# manylinux, pip will try installing maturin from the source distribution.
# That source distribution obviously can't depend on maturin, so we're using
# the always available setuptools.
#
# Note that this is really only a workaround for bootstrapping and not suited
# for general purpose packaging, i.e. only building a wheel (as in
# `python setup.py bdist_wheel`) and installing (as in
# `pip install <source dir>` are supported. For creating a source distribution
# for maturin itself use `maturin sdist`.

import os
import shutil
import subprocess
import sys

from setuptools import setup
from setuptools.command.install import install

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


class PostInstallCommand(install):
    """Post-installation for installation mode."""

    def run(self):
        source_dir = os.path.dirname(os.path.abspath(__file__))
        executable_name = "maturin.exe" if sys.platform.startswith("win") else "maturin"

        # Shortcut for development
        existing_binary = os.path.join(source_dir, "target", "debug", executable_name)
        if os.path.isfile(existing_binary):
            source = existing_binary
        else:
            if not shutil.which("cargo"):
                raise RuntimeError(
                    "cargo not found in PATH. Please install rust "
                    "(https://www.rust-lang.org/tools/install) and try again"
                )
            subprocess.check_call(
                ["cargo", "rustc", "--bin", "maturin", "--", "-C", "link-arg=-s"]
            )
            source = os.path.join(source_dir, "target", "debug", executable_name)
        # run this after trying to build with cargo (as otherwise this leaves
        # venv in a bad state: https://github.com/benfred/py-spy/issues/69)
        install.run(self)

        target = os.path.join(self.install_scripts, executable_name)
        os.makedirs(self.install_scripts, exist_ok=True)
        self.copy_file(source, target)
        self.copy_tree(
            os.path.join(source_dir, "maturin"),
            os.path.join(self.root or self.install_lib, "maturin"),
        )


with open("Readme.md") as fp:
    long_description = fp.read()

setup(
    name="maturin",
    author="konstin",
    author_email="konstin@mailbox.org",
    url="https://github.com/pyo3/maturin",
    description="Build and publish crates with pyo3, rust-cpython and cffi bindings as well as rust binaries as "
    "python packages",
    long_description=long_description,
    long_description_content_type="text/markdown",
    version="0.8.0-alpha.1",
    license="MIT OR Apache-2.0",
    python_requires=">=3.5",
    cmdclass={"install": PostInstallCommand, "bdist_wheel": bdist_wheel},
    classifiers=[
        "Software Development :: Build Tools",
        "Programming Language :: Rust",
        "Programming Language :: Python :: Implementation :: CPython",
        "Programming Language :: Python :: Implementation :: PyPy",
    ],
    install_requires=["toml~=0.10.0"],
    zip_safe=False,
)
