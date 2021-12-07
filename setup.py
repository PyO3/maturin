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

import json
import os
import platform
import shutil
import subprocess
import sys

import toml
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
            # https://github.com/PyO3/maturin/pull/398
            cargo = shutil.which("cargo") or shutil.which("cargo.exe")
            if not cargo:
                raise RuntimeError(
                    "cargo not found in PATH. Please install rust "
                    "(https://www.rust-lang.org/tools/install) and try again"
                )

            cargo_args = [
                cargo,
                "rustc",
                "--release",
                "--bin",
                "maturin",
                "--message-format=json",
            ]

            if platform.machine() in ("ppc64le", "ppc64", "powerpc") or (
                sys.platform == "win32" and platform.machine() == "ARM64"
            ):
                cargo_args.extend(
                    ["--no-default-features", "--features=upload,log,human-panic"]
                )
            elif sys.platform.startswith("haiku"):
                # mio and ring doesn't build on haiku
                cargo_args.extend(
                    ["--no-default-features", "--features=log,human-panic"]
                )

            try:
                metadata = json.loads(
                    subprocess.check_output(cargo_args).splitlines()[-2]
                )
            except subprocess.CalledProcessError as exc:
                raise RuntimeError("build maturin failed:\n" + exc.output.decode())
            print(metadata)
            assert metadata["target"]["name"] == "maturin"
            filenames = metadata["filenames"]
            # somehow on openbsd `filenames` is empty but we can use the
            # `executable` instead, see https://github.com/PyO3/maturin/issues/481
            source = filenames[0] if filenames else metadata["executable"]

        # run this after trying to build with cargo (as otherwise this leaves
        # venv in a bad state: https://github.com/benfred/py-spy/issues/69)
        install.run(self)

        target = os.path.join(self.install_scripts, executable_name)
        os.makedirs(self.install_scripts, exist_ok=True)
        self.copy_file(source, target)
        self.copy_tree(
            os.path.join(source_dir, "maturin"),
            os.path.join(self.install_lib, "maturin"),
        )


with open("Readme.md", encoding="utf-8", errors="ignore") as fp:
    long_description = fp.read()

with open("Cargo.toml") as fp:
    version = toml.load(fp)["package"]["version"]

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
    python_requires=">=3.5",
    cmdclass={"install": PostInstallCommand, "bdist_wheel": bdist_wheel},
    classifiers=[
        "Topic :: Software Development :: Build Tools",
        "Programming Language :: Rust",
        "Programming Language :: Python :: Implementation :: CPython",
        "Programming Language :: Python :: Implementation :: PyPy",
    ],
    install_requires=["toml~=0.10.0"],
    zip_safe=False,
)
