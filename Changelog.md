# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (for the cli, not for the crate).

## [Unreleased]

* **Breaking Change**: Build with `--no-default-features` by default when bootstrapping from sdist in [#1333](https://github.com/PyO3/maturin/pull/1333)
* **Breaking Change**: Remove deprecated `sdist-include` option in `pyproject.toml` in [#1335](https://github.com/PyO3/maturin/pull/1335)
* **Breaking Change**: Remove deprecated `python-source` option in `Cargo.toml` in [#1335](https://github.com/PyO3/maturin/pull/1335)
* **Breaking Change**: Turn `patchelf` version warning into a hard error in [#1335](https://github.com/PyO3/maturin/pull/1335)
* **Breaking Change**: [`uniffi_bindgen` CLI](https://mozilla.github.io/uniffi-rs/tutorial/Prerequisites.html#the-uniffi-bindgen-cli-tool) is required for building `uniffi` bindings wheels in [#1352](https://github.com/PyO3/maturin/pull/1352)
* Add Cargo compile targets configuration for filtering multiple bin targets in [#1339](https://github.com/PyO3/maturin/pull/1339)
* Respect `rustflags` settings in cargo configuration file in [#1405](https://github.com/PyO3/maturin/pull/1405)
* Bump MSRV to 1.63.0 in [#1407](https://github.com/PyO3/maturin/pull/1407)
* Add support for uniffi 0.23 in [#1481](https://github.com/PyO3/maturin/pull/1481)

## [0.14.13] - 2023-02-12

* `maturin develop` now looks for a virtualenv `.venv` in the current or any parent directory if no virtual environment is active.
* Add a new `generate-ci` command to generate CI configuration in [#1456](https://github.com/PyO3/maturin/pull/1456)
* Deprecate `--univeral2` in favor of `universal2-apple-darwin` target in [#1457](https://github.com/PyO3/maturin/pull/1457)
* Raise an error when `Cargo.toml` contains removed python package metadata in [#1471](https://github.com/PyO3/maturin/pull/1471)
* Use `extension_name` instead of `module_name` for CFFI extensions in develop mode in [#1476](https://github.com/PyO3/maturin/pull/1476)

## [0.14.12] - 2023-01-31

* Keep `dev-dependencies` in sdist when there are no path dependencies in [#1441](https://github.com/PyO3/maturin/pull/1441)

## [0.14.11] - 2023-01-31

* Don't package dev-only path dependencies in sdist in [#1435](https://github.com/PyO3/maturin/pull/1435)

## [0.14.10] - 2023-01-13

* Use module name specified by `[package.metadata.maturin]` in [#1409](https://github.com/PyO3/maturin/pull/1409)

## [0.14.9] - 2023-01-10

* Don't pass `MACOSX_DEPLOYMENT_TARGET` when query default value from rustc in [#1395](https://github.com/PyO3/maturin/pull/1395)

## [0.14.8] - 2022-12-31

* Add support for packaging multiple pure Python packages in [#1378](https://github.com/PyO3/maturin/pull/1378)
* Fallback to sysconfig interpreters for pyo3 bindings in [#1381](https://github.com/PyO3/maturin/pull/1381)

## [0.14.7] - 2022-12-20

* Add workspace lock file to sdist as a fallback in [#1362](https://github.com/PyO3/maturin/pull/1362)

## [0.14.6] - 2022-12-13

* Allow Rust crate to be placed outside of the directory containing `pyproject.toml` in [#1347](https://github.com/PyO3/maturin/pull/1347)
* Disallow uniffi bin bindings in [#1353](https://github.com/PyO3/maturin/pull/1353)
* Update bundled Python sysconfigs for Linux and macOS

## [0.14.5] - 2022-12-08

* Support `SOURCE_DATE_EPOCH` when building wheels in [#1334](https://github.com/PyO3/maturin/pull/1334)
* Fix sdist when all Cargo workspace members are excluded in [#1343](https://github.com/PyO3/maturin/pull/1343)

## [0.14.4] - 2022-12-05

* Expanded architecture support for FreeBSD, NetBSD and OpenBSD in [#1318](https://github.com/PyO3/maturin/pull/1318)
* Better error message when upload failed with status code 403 in [#1323](https://github.com/PyO3/maturin/pull/1323)

## [0.14.3] - 2022-12-01

* Bump MSRV to 1.62.0 in [#1297](https://github.com/PyO3/maturin/pull/1297)
* Fix build error when required features of bin target isn't enabled in [#1299](https://github.com/PyO3/maturin/pull/1299)
* Fix wrong platform tag when building in i386 docker container on x86_64 host in [#1301](https://github.com/PyO3/maturin/pull/1301)
* Fix wrong platform tag when building in armv7 docker container on aarch64 host in [#1303](https://github.com/PyO3/maturin/pull/1303)
* Add Solaris operating system support in [#1310](https://github.com/PyO3/maturin/pull/1310)
* Add armv6 and armv7 target support for FreeBSD in [#1312](https://github.com/PyO3/maturin/pull/1312)
* Add riscv64 and powerpc target support for FreeBSD in [#1313](https://github.com/PyO3/maturin/pull/1313)
* Fix powerpc64 and powerpc64le Python wheel platform tag for FreeBSD in [#1313](https://github.com/PyO3/maturin/pull/1313)

## [0.14.2] - 2022-11-24

* Tighten src-layout detection logic in [#1281](https://github.com/PyO3/maturin/pull/1282)
* Fix generating pep517 sdist for src-layout in [#1288](https://github.com/PyO3/maturin/pull/1288)
* Deprecate `python-source` option in `Cargo.toml` in favor of the one in `pyproject.toml` in [#1291](https://github.com/PyO3/maturin/pull/1291)
* Fix auditwheel with read-only libraries in [#1292](https://github.com/PyO3/maturin/pull/1292)

## [0.14.1] - 2022-11-20

* Downgrade `cargo_metadata` to 0.15.0 to fix `maturin build` on old Rust versions like 1.48.0 in [#1279](https://github.com/PyO3/maturin/pull/1279)

## [0.14.0] - 2022-11-19

* **Breaking Change**: Remove support for specifying python package metadata in `Cargo.toml` in [#1200](https://github.com/PyO3/maturin/pull/1200).
  Python package metadata should be specified in the `project` section of `pyproject.toml` instead as [PEP 621](https://peps.python.org/pep-0621/) specifies.
* Initial support for shipping bin targets as wasm32-wasi binaries that are run through wasmtime in [#1107](https://github.com/PyO3/maturin/pull/1107). 
  Note that wasmtime currently only support the five most popular platforms and that wasi binaries have restrictions when interacting with the host.
  Usage is by setting `--target wasm32-wasi`.
* Add support for python first [`src` project layout](https://py-pkgs.org/04-package-structure.html#the-source-layout) in [#1185](https://github.com/PyO3/maturin/pull/1185)
* Add `--src` option to generate src layout for mixed Python/Rust projects in [#1189](https://github.com/PyO3/maturin/pull/1189)
* Add Python metadata support for `license-file` field of `Cargo.toml` in [#1195](https://github.com/PyO3/maturin/pull/1195)
* Upgrade to clap 4.0 in [#1197](https://github.com/PyO3/maturin/pull/1197). This bumps MSRV to 1.61.0.
* Remove `workspace.members` in `Cargo.toml` from sdist if there isn't any path dependency in #[1227](https://github.com/PyO3/maturin/pull/1227)
* Fix auditwheel `libpython` check on Python 3.7 and older versions in [#1229](https://github.com/PyO3/maturin/pull/1229)
* Use generic tags when `sys.implementation.name` != `platform.python_implementation()` in [#1232](https://github.com/PyO3/maturin/pull/1232).
  Fixes the compatibility tags for Pyston.
* Set default macOS deployment target version if `MACOSX_DEPLOYMENT_TARGET` isn't specified in [#1251](https://github.com/PyO3/maturin/pull/1251)
* Add support for 32-bit x86 FreeBSD target in [#1254](https://github.com/PyO3/maturin/pull/1254)
* Add `[tool.maturin.include]` and `[tool.maturin.exclude]` and deprecate `[tool.maturin.sdist-include]` [#1255](https://github.com/PyO3/maturin/pull/1255)
* Ignore sdist tar ball instead of error out in [#1259](https://github.com/PyO3/maturin/pull/1259)
* Add support for [`uniffi`](https://github.com/mozilla/uniffi-rs) bindings in [#1275](https://github.com/PyO3/maturin/pull/1275)

## [0.13.7] - 2022-10-29

* Fix macOS `LC_ID_DYLIB` for abi3 wheels in [#1208](https://github.com/PyO3/maturin/pull/1208)
* Pass `--locked` to Cargo when bootstrap from sdist in [#1212](https://github.com/PyO3/maturin/pull/1212)
* Fix build for Python 3.11 on Windows in [#1222](https://github.com/PyO3/maturin/pull/1222)

## [0.13.6] - 2022-10-08

* Fix `maturin develop` in Windows conda virtual environment in [#1146](https://github.com/PyO3/maturin/pull/1146)
* Fix build for crate using `pyo3` and `build.rs` without `cdylib` crate type in [#1150](https://github.com/PyO3/maturin/pull/1150)
* Fix build on some 32-bit platform by downgrading `indicatif` in [#1163](https://github.com/PyO3/maturin/pull/1163)
* Include `Cargo.lock` by default in source distribution in [#1170](https://github.com/PyO3/maturin/pull/1170)

## [0.13.5] - 2022-09-27

* Fix resolving crate name bug in [#1142](https://github.com/PyO3/maturin/pull/1142)

## [0.13.4] - 2022-09-27

* Fix `Cargo.toml` in new project template in [#1109](https://github.com/PyO3/maturin/pull/1109)
* Fix `maturin develop` on Windows when using Python installed from msys2 in [#1112](https://github.com/PyO3/maturin/pull/1112)
* Fix duplicated `Cargo.toml` of local dependencies in sdist in [#1114](https://github.com/PyO3/maturin/pull/1114)
* Add support for Cargo workspace dependencies inheritance in [#1123](https://github.com/PyO3/maturin/pull/1123)
* Add support for Cargo workspace metadata inheritance in [#1131](https://github.com/PyO3/maturin/pull/1131)
* Use `goblin` instead of shelling out to `patchelf` to get rpath in [#1139](https://github.com/PyO3/maturin/pull/1139)

## [0.13.3] - 2022-09-15

* Allow user to override default Emscripten settings in [#1059](https://github.com/PyO3/maturin/pull/1059)
* Enable `--crate-type cdylib` on Rust 1.64.0 in [#1060](https://github.com/PyO3/maturin/pull/1060)
* Update MSRV to 1.59.0 in [#1071](https://github.com/PyO3/maturin/pull/1071)
* Fix abi3 wheel build when no Python interpreters found in [#1072](https://github.com/PyO3/maturin/pull/1072)
* Add `zig ar` support in [#1073](https://github.com/PyO3/maturin/pull/1073)
* Fix sdist build for optional path dependencies in [#1084](https://github.com/PyO3/maturin/pull/1084)
* auditwheel: find dylibs in Cargo target directory in [#1092](https://github.com/PyO3/maturin/pull/1092)
* Add library search paths in Cargo target directory to rpath in editable mode on Linux in [#1094](https://github.com/PyO3/maturin/pull/1094)
* Remove default manifest path for `maturin sdist` command in [#1097](https://github.com/PyO3/maturin/pull/1097)
* Fix sdist when `pyproject.toml` isn't in the same dir of `Cargo.toml` in [#1099](https://github.com/PyO3/maturin/pull/1099)
* Change readme and license paths in `pyproject.toml` to be relative to `pyproject.toml` in [#1100](https://github.com/PyO3/maturin/pull/1100).
  It's technically a **breaking change**, but previously it doesn't work properly.
* Add python source files specified in pyproject.toml to sdist in [#1102](https://github.com/PyO3/maturin/pull/1102)
* Change `sdist-include` paths to be relative to `pyproject.toml` in [#1103](https://github.com/PyO3/maturin/pull/1103)

## [0.13.2] - 2022-08-14

* Deprecate manylinux 2010 support in [#858](https://github.com/PyO3/maturin/pull/858).
  The [manylinux](https://github.com/pypa/manylinux) project already dropped its support
  and the rustc compiler will [drop glibc 2.12 support in 1.64.0](https://blog.rust-lang.org/2022/08/01/Increasing-glibc-kernel-requirements.html).
* Add Linux mips64el architecture support in [#1023](https://github.com/PyO3/maturin/pull/1023)
* Add Linux mipsel architecture support in [#1024](https://github.com/PyO3/maturin/pull/1024)
* Add Linux 32-bit powerpc architecture support in [#1026](https://github.com/PyO3/maturin/pull/1026)
* Add Linux sparc64 architecture support in [#1027](https://github.com/PyO3/maturin/pull/1027)
* Add PEP 440 local version identifier support in [#1037](https://github.com/PyO3/maturin/pull/1037)
* Fix inconsistent `Cargo.toml` and `pyproject.toml` path handling in [#1043](https://github.com/PyO3/maturin/pull/1043)
* Find python module next to `pyproject.toml` if `pyproject.toml` exists in [#1044](https://github.com/PyO3/maturin/pull/1044).
  It's technically a **breaking change**, but previously it doesn't work properly
  if the directory containing `pyproject.toml` isn't recognized as project root.
* Add `python-source` option to `[tool.maturin]` section of pyproject.toml in [#1046](https://github.com/PyO3/maturin/pull/1046)
* Deprecate support for specifying python metadata in `Cargo.toml` in [#1048](https://github.com/PyO3/maturin/pull/1048).
  Please migrate to [PEP 621](https://peps.python.org/pep-0621/) instead.
* Change `python-source` to be relative to the file specifies it in [#1049](https://github.com/PyO3/maturin/pull/1049)
* Change `data` to be relative to the file specifies it in [#1051](https://github.com/PyO3/maturin/pull/1051)
* Don't reinstall dependencies in `maturin develop` in [#1052](https://github.com/PyO3/maturin/pull/1052)
* Find `pyproject.toml` in parent directories of `Cargo.toml` in [#1054](https://github.com/PyO3/maturin/pull/1054)

## [0.13.1] - 2022-07-26

* Add 64-bit RISC-V support by felixonmars in [#1001](https://github.com/PyO3/maturin/pull/1001)
* Add support for invoking with `python3 -m maturin` in [#1008](https://github.com/PyO3/maturin/pull/1008)
* Fix detection of optional dependencies when declaring `features` in `pyproject.toml` in [#1014](https://github.com/PyO3/maturin/pull/1014)
* Respect user specified Rust target in `maturin develop` in [#1016](https://github.com/PyO3/maturin/pull/1016)
* Use `cargo rustc --crate-type cdylib` on Rust nightly/dev channel in [#1020](https://github.com/PyO3/maturin/pull/1020)

## [0.13.0] - 2022-07-09

* **Breaking Change**: Drop support for python 3.6, which is end of life in [#945](https://github.com/PyO3/maturin/pull/945)
* **Breaking Change**: Don't build source distribution by default in `maturin build` command in [#955](https://github.com/PyO3/maturin/pull/955), `--no-sdist` option is replaced by `--sdist`
* **Breaking Change**: maturin no longer search for python interpreters by default and only build for current interpreter in `PATH` in [#964](https://github.com/PyO3/maturin/pull/964)
* **Breaking Change**: Removed `--cargo-extra-args` and `--rustc-extra-args` options in [#972](https://github.com/PyO3/maturin/pull/972). You can now pass all common `cargo build` arguments directly to `maturin build`
* **Breaking Change**: `--repository-url` option in `upload` command no longer accepts plain repository name, full url required and `-r` short option moved to `--repository` in [#987](https://github.com/PyO3/maturin/pull/987)
* Add support for building with multiple binary targets in [#948](https://github.com/PyO3/maturin/pull/948)
* Add a `--target` option to `maturin list-python` command in [#957](https://github.com/PyO3/maturin/pull/957)
* Add support for using bundled python sysconfigs for PyPy when abi3 feature is enabled in [#958](https://github.com/PyO3/maturin/pull/958)
* Add support for cross compiling PyPy wheels when abi3 feature is enabled in [#963](https://github.com/PyO3/maturin/pull/963)
* Add `--find-interpreter` option to `build` and `publish` commands to search for python interpreters in [#964](https://github.com/PyO3/maturin/pull/964)
* Infer target triple from `ARCHFLAGS` for macOS to be compatible with `cibuildwheel` in [#967](https://github.com/PyO3/maturin/pull/967)
* Expose commonly used Cargo CLI options in `maturin build` command in [#972](https://github.com/PyO3/maturin/pull/972)
* Add support for `wasm32-unknown-emscripten` target in [#974](https://github.com/PyO3/maturin/pull/974)
* Allow overriding platform release version using env var in [#975](https://github.com/PyO3/maturin/pull/975)
* Fix `maturin develop` for arm64 Python on M1 Mac when default toolchain is x86_64 in [#980](https://github.com/PyO3/maturin/pull/980)
* Add `--repository` option to `maturin upload` command in [#987](https://github.com/PyO3/maturin/pull/987)
* Only lookup bundled Python sysconfig when interpreters aren't specified as file path in [#988](https://github.com/PyO3/maturin/pull/988)
* Find CPython upper to 3.12 and PyPy upper to 3.10 in [#993](https://github.com/PyO3/maturin/pull/993)
* Add short alias `maturin b` for `maturin build` and `maturin dev` for `maturin develop` subcommands in [#994](https://github.com/PyO3/maturin/pull/994)

## [0.12.20] - 2022-06-15

* Fix incompatibility with cibuildwheel for 32-bit Windows in [#951](https://github.com/PyO3/maturin/pull/951)
* Don't require `pip` error messages to be utf-8 encoding in [#953](https://github.com/PyO3/maturin/pull/953)
* Compare minimum python version requirement between `requires-python` and bindings crate in [#954](https://github.com/PyO3/maturin/pull/954)
* Set `PYO3_PYTHON` env var for PyPy when abi3 is enabled in [#960](https://github.com/PyO3/maturin/pull/960)
* Add sysconfigs for x64 Windows PyPy in [#962](https://github.com/PyO3/maturin/pull/962)
* Add support for Linux armv6l in [#966](https://github.com/PyO3/maturin/pull/966)
* Fix auditwheel bundled shared libs directory name in [#969](https://github.com/PyO3/maturin/pull/969)

## [0.12.19] - 2022-06-05

* Fix Windows Store install detection in [#949](https://github.com/PyO3/maturin/pull/949)
* Filter Python interpreters by target pointer width on Windows in [#950](https://github.com/PyO3/maturin/pull/950)

## [0.12.18] - 2022-05-29

* Add support for building bin bindings wheels with multiple platform tags in [#928](https://github.com/PyO3/maturin/pull/928)
* Skip auditwheel for non-compliant linux environment automatically in [#931](https://github.com/PyO3/maturin/pull/931)
* Fix abi3 wheel build issue when no Python interpreters found on host in [#933](https://github.com/PyO3/maturin/pull/933)
* Add Python 3.11 sysconfigs for Linux, macOS and Windows in [#934](https://github.com/PyO3/maturin/pull/934)
* Add Python 3.11 sysconfig for arm64 Windows in [#936](https://github.com/PyO3/maturin/pull/936)
* Add network proxy support to upload command in [#939](https://github.com/PyO3/maturin/pull/939)
* Fix python interpreter detection on arm64 Windows in [#940](https://github.com/PyO3/maturin/pull/940)
* Fallback to `py -X.Y` when `pythonX.Y` cannot be found on Windows in [#943](https://github.com/PyO3/maturin/pull/943)
* Auto-detect Python Installs from Microsoft Store in [#944](https://github.com/PyO3/maturin/pull/944)
* Add bindings detection to bin targets in [#938](https://github.com/PyO3/maturin/pull/938)

## [0.12.17] - 2022-05-18

* Don't consider compile to i686 on x86_64 Windows cross compiling in [#923](https://github.com/PyO3/maturin/pull/923)
* Accept `-i x.y` and `-i python-x.y` in `maturin build` command in [#925](https://github.com/PyO3/maturin/pull/925)

## [0.12.16] - 2022-05-16

* Add Linux armv7l python sysconfig in [#901](https://github.com/PyO3/maturin/pull/901)
* Add NetBSD python sysconfig in [#903](https://github.com/PyO3/maturin/pull/903)
* Update 'replace_needed' to reduce total calls to 'patchelf' in [#905](https://github.com/PyO3/maturin/pull/905)
* Add wheel data support in [#906](https://github.com/PyO3/maturin/pull/906)
* Allow use python interpreters from bundled sysconfig when not cross compiling in [#907](https://github.com/PyO3/maturin/pull/907)
* Use setuptools-rust for bootstrapping in [#909](https://github.com/PyO3/maturin/pull/909)
* Allow setting the publish repository URL via `MATURIN_REPOSITORY_URL` in [#913](https://github.com/PyO3/maturin/pull/913)
* Allow stubs-only mixed project layout in [#914](https://github.com/PyO3/maturin/pull/914)
* Allow setting the publish user name via `MATURIN_USERNAME` in [#915](https://github.com/PyO3/maturin/pull/915)
* Add Windows python sysconfig in [#917](https://github.com/PyO3/maturin/pull/917)
* Add support for `generate-import-lib` feature of pyo3 in [#918](https://github.com/PyO3/maturin/pull/918)
* Integrate [`cargo-xwin`](https://github.com/messense/cargo-xwin) for cross compiling to Windows MSVC targets in [#919](https://github.com/PyO3/maturin/pull/919)

## [0.12.15] - 2022-05-07

* Re-export `__all__` for pure Rust projects in [#886](https://github.com/PyO3/maturin/pull/886)
* Stop setting `RUSTFLAGS` environment variable to an empty string in [#887](https://github.com/PyO3/maturin/pull/887)
* Add hardcoded well-known sysconfigs for effortless cross compiling in [#896](https://github.com/PyO3/maturin/pull/896)
* Add support for `PYO3_CONFIG_FILE` in [#899](https://github.com/PyO3/maturin/pull/899)

## [0.12.14] - 2022-04-25

* Fix PyPy pep517 build when abi3 feature is enabled in [#883](https://github.com/PyO3/maturin/pull/883)

## [0.12.13] - 2022-04-25

* Stop setting `PYO3_NO_PYTHON` environment variable for pyo3 0.16.4 and later in [#875](https://github.com/PyO3/maturin/pull/875)
* Build Windows abi3 wheels for `pyo3` 0.16.4 and later versions with `generate-abi3-import-lib` feature enabled no longer require a Python interpreter in [#879](https://github.com/PyO3/maturin/pull/879)

## [0.12.12] - 2022-04-07

* Migrate docker image to GitHub container registry in [#845](https://github.com/PyO3/maturin/pull/845)
* Change mixed rust/python template project layout for new projects in [#855](https://github.com/PyO3/maturin/pull/855)
* Automatically include license files in `.dist-info/license_files` following PEP 639 in [#862](https://github.com/PyO3/maturin/pull/862)
* Bring back multiple values support for `--interpreter` option in [#873](https://github.com/PyO3/maturin/pull/873)
* Update the default edition to 2021 for new projects by sa- in [#874](https://github.com/PyO3/maturin/pull/874)
* Drop `python3.6` from `ghcr.io/pyo3/maturin` docker image.

## [0.12.11] - 2022-03-15

* Package license files in `.dist-info/license_files` following PEP 639 in [#837](https://github.com/PyO3/maturin/pull/837)
* Stop testing Python 3.6 on CI since it's already EOL in [#840](https://github.com/PyO3/maturin/pull/840)
* Update workspace members for sdist local dependencies in [#844](https://github.com/PyO3/maturin/pull/844)
* Migrate docker image to github container registry in [#845](https://github.com/PyO3/maturin/pull/845)
* Remove `PYO3_NO_PYTHON` hack for Windows in [#848](https://github.com/PyO3/maturin/pull/848)
* Remove Windows abi3 python lib link hack in [#851](https://github.com/PyO3/maturin/pull/851)
* Add `-r` option as a short alias for `--release` in [#854](https://github.com/PyO3/maturin/pull/854)

## [0.12.10] - 2022-03-09

* Add support for `pyo3-ffi` by ijl in [#804](https://github.com/PyO3/maturin/pull/804)
* Defaults to `musllinux_1_2` for musl target if it's not bin bindings in [#808](https://github.com/PyO3/maturin/pull/808)
* Remove support for building only sdist via `maturin build -i` in [#813](https://github.com/PyO3/maturin/pull/813), use `maturin sdist` instead.
* Add macOS target support for `--zig` in [#817](https://github.com/PyO3/maturin/pull/817)
* Migrate Python dependency `toml` to `tomllib` / `tomli` by Contextualist in [#821](https://github.com/PyO3/maturin/pull/821)
* Disable auditwheel for PEP 517 build wheel process in [#823](https://github.com/PyO3/maturin/pull/823)
* Lookup existing cffi `header.h` in workspace target directory in [#833](https://github.com/PyO3/maturin/pull/833)
* Fix license line ending in wheel metadata for Windows in [#836](https://github.com/PyO3/maturin/pull/836)

## [0.12.9] - 2022-02-09

* Don't require `pyproject.toml` when cargo manifest is not specified in [#806](https://github.com/PyO3/maturin/pull/806)

## [0.12.8] - 2022-02-08

* Add missing `--version` flag from clap 3.0 upgrade

## [0.12.7] - 2022-02-08

* Add support for using [`zig cc`](https://andrewkelley.me/post/zig-cc-powerful-drop-in-replacement-gcc-clang.html) as linker for easier cross compiling and manylinux compliance in [#756](https://github.com/PyO3/maturin/pull/756)
* Switch from reqwest to ureq to reduce dependencies in [#767](https://github.com/PyO3/maturin/pull/767)
* Fix missing Python submodule in wheel in [#772](https://github.com/PyO3/maturin/pull/772)
* Add support for specifying cargo manifest path in pyproject.toml in [#781](https://github.com/PyO3/maturin/pull/781)
* Add support for passing arguments to pep517 command via `MATURIN_PEP517_ARGS` env var in [#786](https://github.com/PyO3/maturin/pull/786)
* Fix auditwheel `No such file or directory` error when `LD_LIBRARY_PATH` contains non-existent paths in [#794](https://github.com/PyO3/maturin/pull/794)

## [0.12.6] - 2021-12-31

* Add support for repairing cross compiled linux wheels in [#754](https://github.com/PyO3/maturin/pull/754)
* Add support for `manylinux_2_28` and `manylinux_2_31` in [#755](https://github.com/PyO3/maturin/pull/755)
* Remove existing so file first in `maturin develop` command to avoid triggering SIGSEV in running process in [#760](https://github.com/PyO3/maturin/pull/760)

## [0.12.5] - 2021-12-20

* Fix docs for `new` and `init` commands in `maturin --help` in [#734](https://github.com/PyO3/maturin/pull/734)
* Add support for x86_64 Haiku in [#735](https://github.com/PyO3/maturin/pull/735)
* Fix undefined auditwheel policy panic in [#740](https://github.com/PyO3/maturin/pull/740)
* Fix sdist upload for packages where the pkgname contains multiple underscores in [#741](https://github.com/PyO3/maturin/pull/741)
* Implement auditwheel repair with patchelf in [#742](https://github.com/PyO3/maturin/pull/742)
* Add `Cargo.lock` to sdist when `--locked` or `--frozen` specified in [#749](https://github.com/PyO3/maturin/pull/749)
* Infer readme file if not specified in [#751](https://github.com/PyO3/maturin/pull/751)

## [0.12.4] - 2021-12-06

* Add a `maturin init` command as a companion to `maturin new` in [#719](https://github.com/PyO3/maturin/pull/719)
* Don't package non-path-dep crates in sdist for workspaces in [#720](https://github.com/PyO3/maturin/pull/720)
* Build release packages with `password-storage` feature in [#725](https://github.com/PyO3/maturin/pull/725)
* Add support for x86_64 DargonFly BSD in [#727](https://github.com/PyO3/maturin/pull/727)
* Add a Python import hook in [#729](https://github.com/PyO3/maturin/pull/729)
* Allow pip warnings in `maturin develop` command in [#732](https://github.com/PyO3/maturin/pull/732)

## [0.12.3] - 2021-11-29

* Use platform tag from `sysconfig.platform` on non-portable Linux in [#709](https://github.com/PyO3/maturin/pull/709)
* Consider current machine architecture when generating platform tags for abi3
  wheels on linux in [#709](https://github.com/PyO3/maturin/pull/709)
* Revert back to Rust 2018 edition in [#710](https://github.com/PyO3/maturin/pull/710)
* Warn missing `cffi` package dependency in [#711](https://github.com/PyO3/maturin/pull/711)
* Add support for Illumos in [#712](https://github.com/PyO3/maturin/pull/712)
* Account for `MACOSX_DEPLOYMENT_TARGET` env var in wheel platform tag in [#716](https://github.com/PyO3/maturin/pull/716)

## [0.12.2] - 2021-11-26

* Add support for excluding files from wheels by `.gitignore` in [#695](https://github.com/PyO3/maturin/pull/695)
* Fix `pip install maturin` on OpenBSD 6.8 in [#697](https://github.com/PyO3/maturin/pull/697)
* Add support for x86, x86_64 and aarch64 on NetBSD in [#704](https://github.com/PyO3/maturin/pull/704)
* Add a `maturin new` command for bootstrapping new projects in [#705](https://github.com/PyO3/maturin/pull/705)

## [0.12.1] - 2021-11-21

* Add support for cross compiling PyPy wheels in [#687](https://github.com/PyO3/maturin/pull/687)
* Fix `sysconfig.get_platform` parsing for macOS in [#690](https://github.com/PyO3/maturin/pull/690)

## [0.12.0] - 2021-11-19

* Add support for PEP 660 editable installs in [#648](https://github.com/PyO3/maturin/pull/648)
* Publish musllinux_1_1 wheels for maturin in [#651](https://github.com/PyO3/maturin/pull/651)
* Refactor `develop` command to act identical to PEP 660 editable wheels in [#653](https://github.com/PyO3/maturin/pull/653)
* Upgrade to Rust 2021 edition in [#655](https://github.com/PyO3/maturin/pull/655)
* Add support for powerpc64 and powerpc64le on FreeBSD by pkubaj in [#656](https://github.com/PyO3/maturin/pull/656)
* Fix false positive missing pyinit warning on arm64 macOS in [#673](https://github.com/PyO3/maturin/pull/673)
* Build without rustls on arm64 Windows by nsait-linaro in [#674](https://github.com/PyO3/maturin/pull/674)
* Publish Windows arm64 wheels to PyPI by nsait-linaro in [#675](https://github.com/PyO3/maturin/pull/675)
* Add support for building on Windows mingw platforms in [#677](https://github.com/PyO3/maturin/pull/677)
* Allow building for non-abi3 pypy wheels when the abi3 feature is enabled in [#678](https://github.com/PyO3/maturin/pull/678)
* Add support for cross compiling to different operating systems in [#680](https://github.com/PyO3/maturin/pull/680)

## [0.11.5] - 2021-10-13

* Fixed module documentation missing bug of pyo3 bindings in [#639](https://github.com/PyO3/maturin/pull/639)
* Fix musllinux auditwheel wrongly detects libc forbidden link in [#643](https://github.com/PyO3/maturin/pull/643)
* Fix finding conda Python interpreters on Windows by RobertColton in [#644](https://github.com/PyO3/maturin/pull/644)
* Fix Unicode metadata when uploading to PyPI in [#645](https://github.com/PyO3/maturin/pull/645)
* Fix incorrectly folded long `Summary` metadata
* Fix cross compilation for Python 3.10 in [#646](https://github.com/PyO3/maturin/pull/646)

## [0.11.4] - 2021-09-28

* Autodetect PyPy executables in [#617](https://github.com/PyO3/maturin/pull/617)
* auditwheel: add `libz.so.1` to whitelisted libraries in [#625](https://github.com/PyO3/maturin/pull/625)
* auditwheel: detect musl libc in [#629](https://github.com/PyO3/maturin/pull/629)
* Fixed Python 3.10 and later versions detection on Windows in [#630](https://github.com/PyO3/maturin/pull/630)
* Install entrypoint scripts in `maturin develop` command in [#633](https://github.com/PyO3/maturin/pull/633) and [#634](https://github.com/PyO3/maturin/pull/634)
* Add support for installing optional dependencies in `maturin develop` command in [#635](https://github.com/PyO3/maturin/pull/635)
* Fixed build error when `manylinux`/`compatibility` options is specified in `pyproject.toml` in [#637](https://github.com/PyO3/maturin/pull/637)

## [0.11.3] - 2021-08-25

* Add path option for Python source in [#584](https://github.com/PyO3/maturin/pull/584)
* Add auditwheel support for musllinux in [#597](https://github.com/PyO3/maturin/pull/597)
* `[tool.maturin]` options from `pyproject.toml` will be used automatically in [#605](https://github.com/PyO3/maturin/pull/605)
* Skip unavailable Python interpreters from pyenv in [#609](https://github.com/PyO3/maturin/pull/609)

## [0.11.2] - 2021-07-20

* Use UTF-8 encoding when reading `pyproject.toml` by domdfcoding in [#588](https://github.com/PyO3/maturin/pull/588)
* Use Cargo's `repository` field as `Source Code` in project URL in [#590](https://github.com/PyO3/maturin/pull/590)
* Fold long header fields in Python metadata in [#594](https://github.com/PyO3/maturin/pull/594)
* Fix `maturin develop` for PyPy on Unix in [#596](https://github.com/PyO3/maturin/pull/596)

## [0.11.1] - 2021-07-10

* Fix sdist error when VCS has uncommitted renamed files in [#585](https://github.com/PyO3/maturin/pull/585)
* Add `maturin completions <shell>` command to generate shell completions in [#586](https://github.com/PyO3/maturin/pull/586)

## [0.11.0] - 2021-07-04

* Add support for reading metadata from [PEP 621](https://www.python.org/dev/peps/pep-0621/) project table in `pyproject.toml` in [#555](https://github.com/PyO3/maturin/pull/555)
* Users should migrate away from the old `[package.metadata.maturin]` table of `Cargo.toml` to this new `[project]` table of `pyproject.toml`
* Add PEP 656 musllinux support in [#543](https://github.com/PyO3/maturin/pull/543)
* `--manylinux` is now called `--compatibility` and supports musllinux
* The pure rust install layout changed from just the shared library to a python module that reexports the shared library. This should have now observable consequences for users of the created wheel expect that `my_project.my_project` is now also importable (and equal to just `my_project`)
* Add support for packaging type stubs in pure Rust project layout in [#567](https://github.com/PyO3/maturin/pull/567)
* Support i386 on OpenBSD in [#568](https://github.com/PyO3/maturin/pull/568)
* Support Aarch64 on OpenBSD in [#570](https://github.com/PyO3/maturin/pull/570)
* Support Aarch64 on FreeBSD in [#571](https://github.com/PyO3/maturin/pull/571)
* `Cargo.toml`'s `authors` field is now optional per Rust [RFC 3052](https://github.com/rust-lang/rfcs/blob/master/text/3052-optional-authors-field.md) in [#573](https://github.com/PyO3/maturin/pull/573)
* Allow dotted keys in `Cargo.toml` by switch from `toml_edit` to `toml` crate in [#577](https://github.com/PyO3/maturin/pull/577)
* Fix source distribution with local path dependencies on Windows in [#580](https://github.com/PyO3/maturin/pull/580)

## [0.10.6] - 2021-05-21

* Fix corrupted macOS binary release in [#547](https://github.com/PyO3/maturin/pull/547)
* Fix build with the "upload" feature disabled by ravenexp in [#548](https://github.com/PyO3/maturin/pull/548)

## [0.10.5] - 2021-05-21

* Add `manylinux_2_27` support in [#521](https://github.com/PyO3/maturin/pull/521)
* Add support for Windows arm64 target in [#524](https://github.com/PyO3/maturin/pull/524)
* Always output PEP 600 platform tags in [#525](https://github.com/PyO3/maturin/pull/525)
* Fix missing `PyInit_<module_name>` warning with Rust submodule in [#528](https://github.com/PyO3/maturin/pull/528)
* Better cross compiling support for PyO3 binding on Unix in [#454](https://github.com/PyO3/maturin/pull/454)
* Fix s390x architecture support in [#530](https://github.com/PyO3/maturin/pull/530)
* Fix auditwheel panic with s390x wheels in [#532](https://github.com/PyO3/maturin/pull/532)
* Support uploading heterogeneous wheels by ravenexp in [#544](https://github.com/PyO3/maturin/pull/544)
* Warn about `pyproject.toml` missing maturin version constraint in [#545](https://github.com/PyO3/maturin/pull/545)

## [0.10.4] - 2021-04-28

 * Interpreter search now uses python 3.6 to 3.12 in [#495](https://github.com/PyO3/maturin/pull/495)
 * Consider requires-python when searching for interpreters in [#495](https://github.com/PyO3/maturin/pull/495)
 * Support Rust extension as a submodule in mixed Python/Rust projects in [#489](https://github.com/PyO3/maturin/pull/489)

## [0.10.3] - 2021-04-13

 * The `upload` command is now implemented, it is mostly similar to `twine upload`. [#484](https://github.com/PyO3/maturin/pull/484)
 * Interpreter search now uses python 3.6 to 3.12
 * Add basic support for OpenBSD in [#496](https://github.com/PyO3/maturin/pull/496)
 * Fix the PowerPC platform by messense in [#503](https://github.com/PyO3/maturin/pull/503)

## [0.10.2] - 2021-04-03

 * Fix `--target` being silently ignored

## [0.10.1] - 2021-04-03

 * Fix a regression in 0.10.0 that would incorrectly assume we're building for musl instead of gnu by messense in [#487](https://github.com/PyO3/maturin/pull/487)
 * Basic s390x support

## [0.10.0] - 2021-04-02

 * Change manylinux default version based on target arch by messense in [#424](https://github.com/PyO3/maturin/pull/424)
 * Support local path dependencies in source distribution (i.e. you can now package a workspace into an sdist)
 * Set a more reasonable LC_ID_DYLIB entry on macOS by messense [#433](https://github.com/PyO3/maturin/pull/433)
 * Add `--skip-existing` option to publish by messense [#444](https://github.com/PyO3/maturin/pull/444)
 * maturn develop install dependencies automatically by messense [#443](https://github.com/PyO3/maturin/pull/443)
 * Load credential from pypirc using repository name instead of package name by messense [#445](https://github.com/PyO3/maturin/pull/445)
 * Add `manylinux_2_24` support in [#451](https://github.com/PyO3/maturin/pull/451)
 * Improve error message when auditwheel failed to find versioned offending symbols in [#452](https://github.com/PyO3/maturin/pull/452)
 * Add auditwheel test to CI in [#455](https://github.com/PyO3/maturin/pull/455)
 * Fix sdist transitive path dependencies.
 * auditwheel choose higher priority tag when possible in [#456](https://github.com/PyO3/maturin/pull/456), dropped `auditwheel` Cargo feature.
 * develop now writes an [INSTALLER](https://packaging.python.org/specifications/recording-installed-packages/#the-installer-file) file
 * develop removes an old .dist-info directory if it exists before installing the new one
 * Fix wheels for PyPy on windows containing extension modules with incorrect names. [#482](https://github.com/PyO3/maturin/pull/482)

## [0.9.4] - 2021-02-18

* Fix building a bin with musl

## [0.9.3]

* CI failure

## [0.9.2] - 2021-02-17

 * Escape version in wheel metadata by messense in [#420](https://github.com/PyO3/maturin/pull/420)
 * Set executable bit on shared library by messense in [#421](https://github.com/PyO3/maturin/pull/421)
 * Rename `classifier` to `classifiers` for pypi compatibility. The old `classifier` is still available and now also works with pypi
 * Fix building for musl by automatically setting `-C target-feature=-crt-static`

## [0.9.1] - 2021-01-13

 * Error when the `abi3` feature is selected but no minimum version
 * Support building universal2 wheels (x86 and aarch64 in a single file) by messense in [#403](https://github.com/PyO3/maturin/pull/403)
 * Recognize `PYO3_CROSS_LIB_DIR` for cross compiling with abi3 targeting windows.
 * `package.metadata.maturin.classifier` is renamed to `classifiers` by kngwyu in [#416](https://github.com/PyO3/maturin/pull/416)
 * Added more instructions to building complex manylinux setups

## [0.9.0] - 2021-01-10

 * Added support for building abi3 wheels with pyo3 0.13.1
 * Python 3.9 is supported (it should have worked before, but it is now tested on ci)
 * There are 64-bit and aarch64 binary builds for linux and 64-bit builds for windows, mac and freebsd-12-1
 * The auditwheel options have changed to `--manylinux=[off|2010|2014]` with manylinux2010 as default, and optionally `--skip-auditwheel`.
 * Removed Python 3.5 since it is unsupported
 * The default and minimum manylinux version is now manylinux2010
 * restructured text (rst) readmes are now supported, by clbarnes in [#360](https://github.com/PyO3/maturin/pull/360)
 * Allow python 3 interpreter with debuginfo use maturin by inevity in [#370](https://github.com/PyO3/maturin/pull/370)
 * pypirc is checked for credentials by houqp in [#374](https://github.com/PyO3/maturin/pull/374)
 * Added support for PowerPC by mzpqnxow and programmerjake in [#366](https://github.com/PyO3/maturin/pull/366)
 * `project-url` is now a toml dictionary instead of a toml list to conform to the standard
 * No more retry loop when the password was wrong
 * When bootstrapping, also search for `cargo.exe` if `cargo` was not found

## [0.8.3] - 2020-08-17

### Added

 * tox is now supported due to a bugfix in the latest version of tox
 * `[tool.maturin]` now supports `sdist-include = ["path/**/*"]` to
include arbitrary files in source distributions ([#296](https://github.com/PyO3/maturin/pull/296)).
 * Add support for PyO3 `0.12`'s `PYO3_PYTHON` environment variable. [#331](https://github.com/PyO3/maturin/pull/331)

### Fixed

 * Fix incorrectly returning full path (not basename) from PEP 517 `build_sdist` hook. This fixes tox support from maturin's side
 * Packages installed with `maturin develop` are now visible to pip and can be uninstalled with pip

## [0.8.2] - 2020-06-29

### Added

 * Python 3.8 was added to PATH in the docker image by oconnor663 in [#302](https://github.com/PyO3/maturin/pull/302)

## [0.8.1] - 2020-04-30

### Added

 * cffi is installed if it's missing and python is running inside a virtualenv.

## [0.8.0] - 2020-04-03

### Added

 * There is now a binary wheel for aarch64
 * Warn if there are local dependencies

### Fixed

 * Omit author_email if `@` is not found in authors by evandrocoan in [#290](https://github.com/PyO3/maturin/pull/290)

## [0.7.9] - 2020-03-06

### Fixed

 * This release includes binary wheels for mac os

## [0.7.8] - 2020-03-06

### Added

 * Added support from arm, specifically arm7l, aarch64 by ijl in [#273](https://github.com/PyO3/maturin/pull/273)
 * Added support for manylinux2014 by ijl in [#273](https://github.com/PyO3/maturin/pull/273)

### Fixed

 * Remove python 2 from tags by ijl in [#254](https://github.com/PyO3/maturin/pull/254)
 * 32-bit wheels didn't work on linux. This has been fixed by dae in [#250](https://github.com/PyO3/maturin/pull/250)
 * The path of the RECORD file on windows used a backward slash instead of a forward slash

## [0.7.7] - 2019-11-12

### Added

 * The setup.py installer for bootstrapping maturin now checks for cargo instead of failing with a complex error message.
 * Upload errors now show the filesize

### Changed

* maturin's metadata now lists a requirement of python3.5 or later to install.

## [0.7.6] - 2019-09-28

### Changed

 * Only `--features`, `--no-default-features` and `--all-features` in `--cargo-extra-args` are passed to `cargo metadata` when determining the bindings, fixing problems in the previous release with arguments supported by `cargo build` but by `cargo metadata`.

## [0.7.5] - 2019-09-24

### Fixed

 * Fix clippy error to fix publishing from ci

## [0.7.4] - 2019-09-22

### Fixed

 * Fix tests

## [0.7.3] - 2019-09-22

### Fixed

 * Fix building when the bindings crate is behind a feature flag

## [0.7.3] - 2019-09-22

## Removed

 * The manylinux docker container doesn't contain musl anymore. If you're targeting musl, there's no need to use manylinux.

## [0.7.2] - 2019-09-05

### Added

 * Allow cross compilation with cffi and a python interpreter with the host target

### Fixed

 * Renamed a folder to maturin so PEP 517 backend works again.

## [0.7.1] - 2019-08-31

### Added

 * `maturin build --interpreter`/`maturin publish --interpreter` builds only a source distribution.

## [0.7.0] - 2019-08-30

With this release, the name of this project changes from _pyo3-pack_ to _maturin_.

### Added

 * Mixed rust/python layout
 * Added PEP 517 support
 * Added a `maturin sdist` command as workaround for [pypa/pip#6041](https://github.com/pypa/pip/issues/6041)
 * Support settings all applicable fields from the python core metadata specification in Cargo.toml
 * Support for FreeBSD by kxepal [#173](https://github.com/PyO3/maturin/pull/173)

## [0.6.1]

### Fixed

 * Downgraded to structopt 0.2.16 to avoid the yanked 0.2.17

## [0.6.0]

### Added

 * Basic pypy support by ijl [#105](https://github.com/PyO3/maturin/pull/105)

### Removed

 * Python 2 support
 * The custom progress bar was removed and cargo's output is shown instead

## [0.5.0]

### Added

 * Support for conda environments on windows by paddyhoran [#52](https://github.com/PyO3/maturin/pull/52)
 * maturin will generate a header for cffi crates using cbinding, which means you don't need a `build.rs` anymore. The option to provide your own header file using a `build.rs` still exists.
 * The [konstin2/maturin](https://cloud.docker.com/u/konstin2/repository/docker/konstin2/maturin) docker image makes it easy to build fully manylinux compliant wheels. See the readme for usage details.
 * Support for manylinux2010 by ijl [#70](https://github.com/PyO3/maturin/pull/70)
 * The `--manxlinux=[1|1-unchecked|2010|2010-unchecked|off]` option allows to build for manylinux1 and manylinux2010, both with audithweel (`1` or `2010`) and without (`1-unchecked` or `2010-unchecked`), but also for the native linux tag with `off`.

### Changed

 * The `--skip-auditwheel` flag has been deprecated in favor of `--manxlinux=[1|1-unchecked|2010|2010-unchecked|off]`.
 * Switched to rustls. This means the upload feature can be used from the docker container and builds of maturin itself are manylinux compliant when compiled with the musl target.

## [0.4.2] - 2018-12-15

Fixup release because the appveyor failed to release artifacts for windows for 0.4.1.

## [0.4.1] - 2018-12-15

### Added

 * You can now specify [trove classifiers](https://pypi.org/classifiers/) in your Cargo.toml with `package.metadata.maturin.classifier`. Implemented by ijl in [#48](https://github.com/PyO3/maturin/pull/48). Example:
 ```toml
  [package.metadata.maturin]
  classifier = ["Programming Language :: Python"]
  ```

## [0.4.0] - 2018-11-20

### Changed

 * publish defaults to release and strip, unless `--debug` or `--no-strip` are given.

### Added

 * New ci script based on hyperfine which also builds debian packages.

## [0.3.10] - 2018-11-16

### Fixed

 * Fix rust-cpython detection and compilation

## [0.3.9]

### Changed

 * Update reqwest to 0.9.4 which has [seanmonstar/reqwest#374](https://github.com/seanmonstar/reqwest/issues/374) fixed

## [0.3.8]

### Fixed

 * Pin reqwest to 0.9.2 to work around [seanmonstar/reqwest#374](https://github.com/seanmonstar/reqwest/issues/374)

## [0.3.7]

### Fixed

 * Added cargo lock to project [#9](https://github.com/PyO3/maturin/issues/9)

## [0.3.6]

With deflate and the strip options, the wheels get about 25x smaller:

wheel | baseline | deflate | strip + deflate
-|-|-|-
get_fourtytwo-2.0.1-cp36-cp36m-manylinux1_x86_64.whl | 2,8M | 771K | 102K
hello_world-0.1.0-py2.py3-none-manylinux1_x86_64.whl | 3,9M | 1,1M | 180K
points-0.1.0-py2.py3-none-manylinux1_x86_64.whl | 2,8M | 752K | 85K

### Added

 * `--strip` by ijl [#7](https://github.com/PyO3/maturin/pull/7)

### Changed

 * Renamed `--bindings-crate` to `--bindings`
 * Use deflate compression for zips by ijl [#6](https://github.com/PyO3/maturin/pull/6)

### Fixed

 * `--target` is now actually used for the wheel compatibility tag

## [0.3.5] - 2018-09-20

### Changed

 * Upgraded to reqwest 0.9

### Fixed

 * "Broken Pipe" with musl builds (through the reqwest upgrade)

## [0.3.4] - 2018-09-18

### Added

 * A `--target` option which behaves like cargo option of the same name

### Changed

 * Musl and auditwheel compliance: Using the new `musl` feature combined with the musl target, you can build completely static binaries. The `password-storage`, which enables keyring integration, is now disabled by default. The Pypi packages are now statically linked with musl so that they are audtiwheel compliant.
 * Replaced `--debug` with `--release`. All builds are now debug by default

## [0.3.3] - 2018-09-17

### Added

 * Builds for i686 linux and mac
 * Builds for maturin as wheel

## Fixed

 * Usage with stable
 * Wrong tags in WHEEL file on non-linux platforms
 * Uploading on windows

## [0.3.1] - 2017-09-14

### Fixed

 * Windows compilation

## [0.3.0] - 2017-09-14

### Added

 * Packaging binaries
 * [Published on pypi](https://pypi.org/project/maturin/). You can now `pip install maturin`
 * A Dockerfile based on manylinux1

### Fixed

 * Travis ci setup builds all types of wheels for linux and mac
 * `--no-default-features --features auditwheel` creates a manylinux compliant binary for maturin

### Changed

 * Replaced elfkit with goblin

## [0.2.0] - 2018-09-03

### Added

 * Cffi support
 * A `develop` subcommand
 * A tox example

### Changed

 * Show a progress bar for cargo's compile progress

## 0.1.0 - 2018-08-22

 * Initial Release

[Unreleased]: https://github.com/pyo3/maturin/compare/v0.14.13...HEAD
[0.14.13]: https://github.com/pyo3/maturin/compare/v0.14.12...v0.14.13
[0.14.12]: https://github.com/pyo3/maturin/compare/v0.14.11...v0.14.12
[0.14.11]: https://github.com/pyo3/maturin/compare/v0.14.10...v0.14.11
[0.14.10]: https://github.com/pyo3/maturin/compare/v0.14.9...v0.14.10
[0.14.9]: https://github.com/pyo3/maturin/compare/v0.14.8...v0.14.9
[0.14.8]: https://github.com/pyo3/maturin/compare/v0.14.7...v0.14.8
[0.14.7]: https://github.com/pyo3/maturin/compare/v0.14.6...v0.14.7
[0.14.6]: https://github.com/pyo3/maturin/compare/v0.14.5...v0.14.6
[0.14.5]: https://github.com/pyo3/maturin/compare/v0.14.4...v0.14.5
[0.14.4]: https://github.com/pyo3/maturin/compare/v0.14.3...v0.14.4
[0.14.3]: https://github.com/pyo3/maturin/compare/v0.14.2...v0.14.3
[0.14.2]: https://github.com/pyo3/maturin/compare/v0.14.1...v0.14.2
[0.14.1]: https://github.com/pyo3/maturin/compare/v0.14.0...v0.14.1
[0.14.0]: https://github.com/pyo3/maturin/compare/v0.13.7...v0.14.0
[0.13.7]: https://github.com/pyo3/maturin/compare/v0.13.6...v0.13.7
[0.13.6]: https://github.com/pyo3/maturin/compare/v0.13.5...v0.13.6
[0.13.5]: https://github.com/pyo3/maturin/compare/v0.13.4...v0.13.5
[0.13.4]: https://github.com/pyo3/maturin/compare/v0.13.3...v0.13.4
[0.13.3]: https://github.com/pyo3/maturin/compare/v0.13.2...v0.13.3
[0.13.2]: https://github.com/pyo3/maturin/compare/v0.13.1...v0.13.2
[0.13.1]: https://github.com/pyo3/maturin/compare/v0.13.0...v0.13.1
[0.13.0]: https://github.com/pyo3/maturin/compare/v0.12.20...v0.13.0
[0.12.20]: https://github.com/pyo3/maturin/compare/v0.12.19...v0.12.20
[0.12.19]: https://github.com/pyo3/maturin/compare/v0.12.18...v0.12.19
[0.12.18]: https://github.com/pyo3/maturin/compare/v0.12.17...v0.12.18
[0.12.17]: https://github.com/pyo3/maturin/compare/v0.12.16...v0.12.17
[0.12.16]: https://github.com/pyo3/maturin/compare/v0.12.15...v0.12.16
[0.12.15]: https://github.com/pyo3/maturin/compare/v0.12.14...v0.12.15
[0.12.14]: https://github.com/pyo3/maturin/compare/v0.12.13...v0.12.14
[0.12.13]: https://github.com/pyo3/maturin/compare/v0.12.12...v0.12.13
[0.12.12]: https://github.com/pyo3/maturin/compare/v0.12.11...v0.12.12
[0.12.11]: https://github.com/pyo3/maturin/compare/v0.12.10...v0.12.11
[0.12.10]: https://github.com/pyo3/maturin/compare/v0.12.9...v0.12.10
[0.12.9]: https://github.com/pyo3/maturin/compare/v0.12.8...v0.12.9
[0.12.8]: https://github.com/pyo3/maturin/compare/v0.12.7...v0.12.8
[0.12.7]: https://github.com/pyo3/maturin/compare/v0.12.6...v0.12.7
[0.12.6]: https://github.com/pyo3/maturin/compare/v0.12.5...v0.12.6
[0.12.5]: https://github.com/pyo3/maturin/compare/v0.12.4...v0.12.5
[0.12.4]: https://github.com/pyo3/maturin/compare/v0.12.3...v0.12.4
[0.12.3]: https://github.com/pyo3/maturin/compare/v0.12.2...v0.12.3
[0.12.2]: https://github.com/pyo3/maturin/compare/v0.12.1...v0.12.2
[0.12.1]: https://github.com/pyo3/maturin/compare/v0.12.0...v0.12.1
[0.12.0]: https://github.com/pyo3/maturin/compare/v0.11.5...v0.12.0
[0.11.5]: https://github.com/pyo3/maturin/compare/v0.11.4...v0.11.5
[0.11.4]: https://github.com/pyo3/maturin/compare/v0.11.3...v0.11.4
[0.11.3]: https://github.com/pyo3/maturin/compare/v0.11.2...v0.11.3
[0.11.2]: https://github.com/pyo3/maturin/compare/v0.11.1...v0.11.2
[0.11.1]: https://github.com/pyo3/maturin/compare/v0.11.0...v0.11.1
[0.11.0]: https://github.com/pyo3/maturin/compare/v0.10.6...v0.11.0
[0.10.6]: https://github.com/pyo3/maturin/compare/v0.10.5...v0.10.6
[0.10.5]: https://github.com/pyo3/maturin/compare/v0.10.4...v0.10.5
[0.10.4]: https://github.com/pyo3/maturin/compare/v0.10.3...v0.10.4
[0.10.3]: https://github.com/pyo3/maturin/compare/v0.10.2...v0.10.3
[0.10.2]: https://github.com/pyo3/maturin/compare/v0.10.1...v0.10.2
[0.10.1]: https://github.com/pyo3/maturin/compare/v0.10.0...v0.10.1
[0.10.0]: https://github.com/pyo3/maturin/compare/v0.9.4...v0.10.0
[0.9.4]: https://github.com/pyo3/maturin/compare/v0.9.3...v0.9.4
[0.9.3]: https://github.com/pyo3/maturin/compare/v0.9.2...v0.9.3
[0.9.2]: https://github.com/pyo3/maturin/compare/v0.9.1...v0.9.2
[0.9.1]: https://github.com/pyo3/maturin/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/pyo3/maturin/compare/v0.8.3...v0.9.0
[0.8.3]: https://github.com/pyo3/maturin/compare/v0.8.2...v0.8.3
[0.8.2]: https://github.com/pyo3/maturin/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/pyo3/maturin/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/pyo3/maturin/compare/v0.7.9...v0.8.0
[0.7.9]: https://github.com/pyo3/maturin/compare/v0.7.8...v0.7.9
[0.7.8]: https://github.com/pyo3/maturin/compare/v0.7.7...v0.7.8
[0.7.7]: https://github.com/pyo3/maturin/compare/v0.7.6...v0.7.7
[0.7.6]: https://github.com/pyo3/maturin/compare/v0.7.5...v0.7.6
[0.7.5]: https://github.com/pyo3/maturin/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/pyo3/maturin/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/pyo3/maturin/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/pyo3/maturin/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/pyo3/maturin/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/pyo3/maturin/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/pyo3/maturin/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/pyo3/maturin/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/pyo3/maturin/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/pyo3/maturin/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/pyo3/maturin/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/pyo3/maturin/compare/v0.3.10...v0.4.0
[0.3.10]: https://github.com/pyo3/maturin/compare/v0.3.9...v0.3.10
[0.3.9]: https://github.com/pyo3/maturin/compare/v0.3.8...v0.3.9
[0.3.8]: https://github.com/pyo3/maturin/compare/v0.3.7...v0.3.8
[0.3.7]: https://github.com/pyo3/maturin/compare/v0.3.6...v0.3.7
[0.3.6]: https://github.com/pyo3/maturin/compare/v0.3.5...v0.3.5
[0.3.5]: https://github.com/pyo3/maturin/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/pyo3/maturin/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/pyo3/maturin/compare/v0.3.1...v0.3.3
[0.3.1]: https://github.com/pyo3/maturin/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/pyo3/maturin/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/pyo3/maturin/compare/v0.1.0...v0.2.0
