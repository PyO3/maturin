# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (for the cli, not for the crate).

## [Unreleased]

* Fix docs for `new` and `init` commands in `maturin --help` in [#734](https://github.com/PyO3/maturin/pull/734)
* Add support for x86_64 Haiku in [#735](https://github.com/PyO3/maturin/pull/735)
* Fix undefined auditwheel policy panic in [#740](https://github.com/PyO3/maturin/pull/740)
* Fix sdist upload for packages where the pkgname contains multiple underscores in [#741](https://github.com/PyO3/maturin/pull/741)

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

[Unreleased]: https://github.com/pyo3/maturin/compare/v0.12.4...HEAD
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
