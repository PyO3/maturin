//! This module is taken mostly verbatim from PyPI's `legacy.py` validation logic.
//!
//! <https://github.com/pypi/warehouse/blob/main/warehouse/forklift/legacy.py>

use once_cell::sync::Lazy;
use regex::Regex;

pub(super) static MACOS_PLATFORM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^macosx_(?P<major>\d+)_(?:\d+)_(?P<arch>.+)$").unwrap());

pub(super) static IOS_PLATFORM_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^ios_(?:\d+)_(?:\d+)_(?P<arch>.+)_(?:iphoneos|iphonesimulator)$").unwrap()
});

pub(super) static ANDROID_PLATFORM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^android_(?:\d+)_(?P<arch>.+)$").unwrap());

pub(super) static LINUX_PLATFORM_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?P<libc>(?:many|musl))linux_(?:\d+)_(?:\d+)_(?P<arch>.+)$").unwrap()
});

/// Contains also non-Rust platforms to match `legacy.py` verbatim.
pub(super) static ALLOWED_PLATFORMS: &[&str] = &[
    "any",
    "win32",
    "win_arm64",
    "win_amd64",
    "win_ia64",
    "manylinux1_x86_64",
    "manylinux1_i686",
    "manylinux2010_x86_64",
    "manylinux2010_i686",
    "manylinux2014_x86_64",
    "manylinux2014_i686",
    "manylinux2014_aarch64",
    "manylinux2014_armv7l",
    "manylinux2014_ppc64",
    "manylinux2014_ppc64le",
    "manylinux2014_s390x",
    "linux_armv6l",
    "linux_armv7l",
];

/// Windows platforms: win32 (i686), win_amd64 (x86_64), win_arm64 (aarch64).
///
/// PyPI allows win_ia64 but Rust doesn't support IA64/Itanium.
pub(super) static WINDOWS_ARCHES: &[&str] = &["x86_64", "i686", "aarch64"];

/// Reduced list only containing targets support by both Rust/maturin and PyPI.
pub(super) static MACOS_ARCHES: &[&str] = &["x86_64", "arm64", "i686", "universal2"];

/// Those are actually hardcoded in warehouse in the same way.
pub(super) static MACOS_MAJOR_VERSIONS: &[&str] = &["10", "11", "12", "13", "14", "15"];

pub(super) static IOS_ARCHES: &[&str] = &["arm64", "x86_64"];

pub(super) static ANDROID_ARCHES: &[&str] = &["armeabi_v7a", "arm64_v8a", "x86", "x86_64"];

pub(super) static MANYLINUX_ARCHES: &[&str] = &[
    "x86_64", "i686", "aarch64", "armv7l", "ppc64le", "s390x", "ppc64", "riscv64",
];

pub(super) static MUSLLINUX_ARCHES: &[&str] =
    &["x86_64", "i686", "aarch64", "armv7l", "ppc64le", "s390x"];
