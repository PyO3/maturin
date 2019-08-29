# pyo3 is self bootstraping, however on platforms like alpine linux that aren't
# manylinux, pip will try installing pyo3-pack from the source distribution.
# That source distribution obviously can't depend on pyo3-pack, so we're using
# the always available setuptools.
#
# Note that this is really only a workaround for bootstrapping and not suited
# for general purpose packaging, i.e. only building a wheel (as in
# `python setup.py bdist_wheel`) and installing (as in
# `pip install <source dir>` are supported. For creating a source distribution
# for pyo3-pack itself use `pyo3-pack sdist`.

import os
import subprocess
import sys

import setuptools
from setuptools import setup
from setuptools.command.install import install


class PostInstallCommand(install):
    """Post-installation for installation mode."""

    def run(self):
        source_dir = os.path.dirname(os.path.abspath(__file__))
        executable_name = (
            "pyo3-pack.exe" if sys.platform.startswith("win") else "pyo3-pack"
        )

        # Shortcut for development
        existing_binary = os.path.join(source_dir, "target", "debug", executable_name)
        if os.path.isfile(existing_binary):
            source = existing_binary
        else:
            subprocess.check_call(["cargo", "rustc", "--bin", "pyo3-pack"])
            source = os.path.join(source_dir, "target", "debug", executable_name)
        # run this after trying to build with cargo (as otherwise this leaves
        # venv in a bad state: https://github.com/benfred/py-spy/issues/69)
        install.run(self)

        target = os.path.join(self.install_scripts, executable_name)
        os.makedirs(self.install_scripts, exist_ok=True)
        self.copy_file(source, target)
        self.copy_tree(
            os.path.join(source_dir, "pyo3_pack"),
            os.path.join(self.root or self.install_lib, "pyo3_pack"),
        )


with open("Readme.md") as fp:
    long_description = fp.read()

setup(
    name="pyo3-pack",
    author="konstin",
    author_email="konstin@mailbox.org",
    url="https://github.com/pyo3-pack/pyo3-pack",
    description="Build and publish crates with pyo3, rust-cpython and cffi bindings as well as rust binaries as "
    "python packages",
    long_description=long_description,
    long_description_content_type="text/markdown",
    version="0.7.0-beta.12",
    license="MIT OR Apache-2.0",
    cmdclass={"install": PostInstallCommand},
    classifiers=[
        "Software Development :: Build Tools",
        "Programming Language :: Rust",
        "Programming Language :: Python :: Implementation :: CPython",
        "Programming Language :: Python :: Implementation :: PyPy",
    ],
    # Force the wheel to be platform specific
    # https://stackoverflow.com/a/53463910/3549270
    ext_modules=[setuptools.Extension(name="dummy", sources=[])],
    install_requires=["toml~=0.10.0"],
    zip_safe=False,
)
