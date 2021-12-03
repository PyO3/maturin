# Platform Support

Being built on cargo and rustc, maturin is limited by [rust's platform support](https://doc.rust-lang.org/nightly/rustc/platform-support.html).

## Automated tests

On GitHub actions, windows, macOS and linux are tested, all
on 64-bit x86. FreeBSD is also tested though Cirrus CI, but might get removed at
some point. Since CI is very time intensive to maintain, I'd like to stick to
GitHub action and these three platforms.

## Releases

The following targets are built into wheels and downloadable binaries:

 * windows: 32-bit and 64-bit x86
 * linux 32-bit and 64-bit x86 as well as armv7 and aarch64 (musl)
 * macOS: 64-bit and aarch64

## Other Operating Systems

It should be possible to build maturin and for maturin to build wheels on other platforms supported by rust.
To add a new os, add it in target.rs and, if it doesn't behave like the other unixes, in
`PythonInterpreter::get_tag`. Please also submit the output of `python -m sysconfig` as a file in the `sysconfig` folder.
It's ok to edit setup.py to deactivate default features so `pip install` works, but new platforms should not
require complex workaround in `compile.rs`.

## Architectures

All architectures included in manylinux (aarch64, armv7l, ppc64le, ppc64, i686, x86_64, s390x) are supported.
I'm not sure whether it makes sense to allow architectures that aren't even
supported by [manylinux](https://github.com/pypa/manylinux).

## Python Support

CPython 3.6 to 3.9 are supported and tested on CI, though the entire 3.x series should work.
This will be changed as new python versions are released and others have their end of life.

PyPy 3.6 and later also works.
