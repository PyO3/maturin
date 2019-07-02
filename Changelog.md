# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (for the cli, not for the crate).

## [Unreleased]

### Added

 * Mixed rust/python layout
 * Added PEP 517 support
 * Added a `pyo-pack sdist` command as workaround for [pypa/pip#6041](https://github.com/pypa/pip/issues/6041)

## [0.6.1]

### Fixed

 * Downgraded to structopt 0.2.16 to avoid the yanked 0.2.17

## [0.6.0]

### Added

 * Basic pypy support by ijl [#105](https://github.com/PyO3/pyo3-pack/pull/105)

### Removed

 * Python 2 support
 * The custom progress bar was removed and cargo's output is shown instead

## [0.5.0]

### Added

 * Support for conda environments on windows by paddyhoran [#52](https://github.com/PyO3/pyo3-pack/pull/52)
 * pyo3-pack will generate a header for cffi crates using cbinding, which means you don't need a `build.rs` anymore. The option to provide your own header file using a `build.rs` still exists.
 * The [konstin2/pyo3-pack](https://cloud.docker.com/u/konstin2/repository/docker/konstin2/pyo3-pack) docker image makes it easy to build fully manylinux compliant wheels. See the readme for usage details.
 * Support for manylinux2010 by ijl [#70](https://github.com/PyO3/pyo3-pack/pull/70)
 * The `--manxlinux=[1|1-unchecked|2010|2010-unchecked|off]` option allows to build for manylinux1 and manylinux2010, both with audithweel (`1` or `2010`) and without (`1-unchecked` or `2010-unchecked`), but also for the native linux tag with `off`.

### Changed

 * The `--skip-auditwheel` flag has been deprecated in favor of `--manxlinux=[1|1-unchecked|2010|2010-unchecked|off]`.
 * Switched to rustls. This means the upload feature can be used from the docker container and builds of pyo3-pack itself are manylinux compliant when compiled with the musl target.

## [0.4.2] - 2018-12-15

Fixup release because the appveyor failed to release artifacts for windows for 0.4.1.

## [0.4.1] - 2018-12-15

### Added

 * You can now specify [trove classifiers](https://pypi.org/classifiers/) in your Cargo.toml with `package.metadata.pyo3-pack.classifier`. Implemented by ijl in [#48](https://github.com/PyO3/pyo3-pack/pull/48). Example:
 ```toml
  [package.metadata.pyo3-pack]
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

 * Added cargo lock to project [#9](https://github.com/PyO3/pyo3-pack/issues/9)

## [0.3.6]

With deflate and the strip options, the wheels get about 25x smaller:

wheel | baseline | deflate | strip + deflate
-|-|-|-
get_fourtytwo-2.0.1-cp36-cp36m-manylinux1_x86_64.whl | 2,8M | 771K | 102K
hello_world-0.1.0-py2.py3-none-manylinux1_x86_64.whl | 3,9M | 1,1M | 180K
points-0.1.0-py2.py3-none-manylinux1_x86_64.whl | 2,8M | 752K | 85K

### Added

 * `--strip` by ijl [#7](https://github.com/PyO3/pyo3-pack/pull/7)

### Changed

 * Renamed `--bindings-crate` to `--bindings`
 * Use deflate compression for zips by ijl [#6](https://github.com/PyO3/pyo3-pack/pull/6)

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
 * Builds for pyo3-pack as wheel

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
 * [Published on pypi](https://pypi.org/project/pyo3-pack/). You can now `pip install pyo3-pack`
 * A Dockerfile based on manylinux1

### Fixed

 * Travis ci setup builds all types of wheels for linux and mac
 * `--no-default-features --features auditwheel` creates a manylinux compliant binary for pyo3-pack

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

[Unreleased]: https://github.com/pyo3/pyo3-pack/compare/v0.6.1...HEAD
[0.6.1]: https://github.com/pyo3/pyo3-pack/compare/v0.6.0...0.6.1
[0.6.0]: https://github.com/pyo3/pyo3-pack/compare/v0.5.0...0.6.0
[0.5.0]: https://github.com/pyo3/pyo3-pack/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/pyo3/pyo3-pack/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/pyo3/pyo3-pack/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/pyo3/pyo3-pack/compare/v0.3.10...v0.4.0
[0.3.10]: https://github.com/pyo3/pyo3-pack/compare/v0.3.9...v0.3.10
[0.3.9]: https://github.com/pyo3/pyo3-pack/compare/v0.3.8...v0.3.9
[0.3.8]: https://github.com/pyo3/pyo3-pack/compare/v0.3.7...v0.3.8
[0.3.7]: https://github.com/pyo3/pyo3-pack/compare/v0.3.6...v0.3.7
[0.3.6]: https://github.com/pyo3/pyo3-pack/compare/v0.3.5...v0.3.5
[0.3.5]: https://github.com/pyo3/pyo3-pack/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/pyo3/pyo3-pack/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/pyo3/pyo3-pack/compare/v0.3.1...v0.3.3
[0.3.1]: https://github.com/pyo3/pyo3-pack/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/pyo3/pyo3-pack/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/pyo3/pyo3-pack/compare/v0.1.0...v0.2.0
