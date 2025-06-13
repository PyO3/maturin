//! PyPI compatibility checking for platform tags.
//!
//! This module implements PyPI's platform tag validation rules to ensure wheels
//! are compatible with PyPI's upload requirements. The validation logic is based on
//! warehouse: <https://github.com/pypi/warehouse/blob/main/warehouse/forklift/legacy.py>
//!
//! Differences between PyPI's support and Rust:
//! - Windows: PyPI allows ia64 (win_ia64) but Rust doesn't support IA64/Itanium
//! - macOS: PyPI allows legacy PPC and fat binaries but these are either unsupported by Rust
//!   or created post-build with lipo rather than being direct Rust targets. This is supported by
//!   the virtual universal2-apple-darwin target instead.
//!
//! Supported architectures by platform (intersection of PyPI and Rust/maturin support):
//! - Linux (manylinux): x86_64, i686, aarch64, armv7l, ppc64le, s390x, ppc64
//! - Linux (musllinux): x86_64, i686, aarch64, armv7l, ppc64le, s390x
//! - Windows: x86_64, i686, aarch64
//! - macOS: x86_64, arm64, i686 (Tier 3), universal2 (maturin special target)
//! - iOS: arm64, x86_64 (simulator)
//! - Android: armeabi_v7a (armv7l), arm64_v8a (aarch64), x86 (i686), x86_64

use crate::target::{Os, Target};
use anyhow::{anyhow, bail, Result};
use once_cell::sync::Lazy;
use regex::Regex;

static LINUX_PLATFORM_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?P<libc>(?:many|musl))linux_(?:\d+)_(?:\d+)_(?P<arch>.+)$").unwrap()
});

static MACOS_PLATFORM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^macosx_(?P<major>\d+)_(?:\d+)_(?P<arch>.+)$").unwrap());

static IOS_PLATFORM_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^ios_(?:\d+)_(?:\d+)_(?P<arch>.+)_(?:iphoneos|iphonesimulator)$").unwrap()
});

static ANDROID_PLATFORM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^android_(?:\d+)_(?P<arch>.+)$").unwrap());

static SIMPLE_PLATFORMS: &[&str] = &[
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

static MACOS_ARCHES: &[&str] = &[
    "ppc",
    "ppc64",
    "i386",
    "x86_64",
    "arm64",
    "intel",
    "fat",
    "fat3",
    "fat64",
    "universal",
    "universal2",
];

/// Those are actually hardcoded in warehouse.
static MACOS_MAJOR_VERSIONS: &[&str] = &["10", "11", "12", "13", "14", "15"];

static IOS_ARCHES: &[&str] = &["arm64", "x86_64"];

static ANDROID_ARCHES: &[&str] = &["armeabi_v7a", "arm64_v8a", "x86", "x86_64"];

/// Check for target architectures that we know aren't supported by PyPI to error early.
pub fn is_arch_supported_by_pypi(target: &Target) -> bool {
    let arch = target.target_arch().to_string();
    match target.target_os() {
        Os::Windows => {
            // Windows platforms: win32 (i686), win_amd64 (x86_64), win_arm64 (aarch64).
            // PyPI allows win_ia64 but Rust doesn't support IA64/Itanium
            matches!(&*arch, "x86_64" | "i686" | "aarch64")
        }
        Os::Macos => {
            // macOS uses arm64 in platform tags, but target triple uses aarch64
            let normalized_arch = if arch == "aarch64" { "arm64" } else { &arch };
            // Only allow architectures that Rust/maturin actually supports on macOS.
            // PyPI allows many more (ppc, ppc64, i386, intel, fat*, universal*)
            // but these are either legacy or created post-build with lipo.
            matches!(normalized_arch, "x86_64" | "arm64" | "i686" | "universal2")
        }
        Os::Ios => {
            // iOS uses arm64 in platform tags, but target triple uses aarch64
            let normalized_arch = if arch == "aarch64" { "arm64" } else { &arch };
            // PyPI allows iOS with arm64 and x86_64 (simulator)
            matches!(normalized_arch, "arm64" | "x86_64")
        }
        Os::Linux if target.target_triple().contains("android") => {
            // Android target triples map to specific platform tag architectures
            let android_arch = match arch.as_str() {
                "armv7l" => "armeabi_v7a", // armv7 little-endian
                "aarch64" => "arm64_v8a",
                "i686" => "x86",
                "x86_64" => "x86_64",
                _ => return false,
            };
            // PyPI allows Android with these specific architectures
            matches!(android_arch, "armeabi_v7a" | "arm64_v8a" | "x86" | "x86_64")
        }
        Os::Linux => {
            if target.is_musl_libc() {
                // musllinux allowed architectures
                matches!(
                    arch.as_str(),
                    "x86_64" | "i686" | "aarch64" | "armv7l" | "ppc64le" | "s390x"
                )
            } else {
                // manylinux allowed architectures
                matches!(
                    arch.as_str(),
                    "x86_64" | "i686" | "aarch64" | "armv7l" | "ppc64le" | "s390x" | "ppc64"
                )
            }
        }
        _ => false,
    }
}

/// Validates that a wheel platform tag is allowed by PyPI.
///
/// Based on PyPI warehouse platform tag validation logic.
fn is_platform_tag_allowed_by_pypi(platform_tag: &str) -> bool {
    if SIMPLE_PLATFORMS.contains(&platform_tag) {
        return true;
    }

    // manylinux/musllinux
    if let Some(captures) = LINUX_PLATFORM_RE.captures(platform_tag) {
        let libc = captures.name("libc").unwrap().as_str();
        let arch = captures.name("arch").unwrap().as_str();

        return match libc {
            "musl" => is_musllinux_arch_allowed(arch),
            "many" => is_manylinux_arch_allowed(arch),
            _ => false,
        };
    }

    // macOS
    if let Some(captures) = MACOS_PLATFORM_RE.captures(platform_tag) {
        let major = captures.name("major").unwrap().as_str();
        let arch = captures.name("arch").unwrap().as_str();

        return MACOS_MAJOR_VERSIONS.contains(&major) && MACOS_ARCHES.contains(&arch);
    }

    // iOS
    if let Some(captures) = IOS_PLATFORM_RE.captures(platform_tag) {
        let arch = captures.name("arch").unwrap().as_str();
        return IOS_ARCHES.contains(&arch);
    }

    // Android
    if let Some(captures) = ANDROID_PLATFORM_RE.captures(platform_tag) {
        let arch = captures.name("arch").unwrap().as_str();
        return ANDROID_ARCHES.contains(&arch);
    }

    false
}

fn is_manylinux_arch_allowed(arch: &str) -> bool {
    matches!(
        arch,
        "x86_64" | "i686" | "aarch64" | "armv7l" | "ppc64le" | "s390x" | "ppc64"
    )
}

fn is_musllinux_arch_allowed(arch: &str) -> bool {
    matches!(
        arch,
        "x86_64" | "i686" | "aarch64" | "armv7l" | "ppc64le" | "s390x"
    )
}

/// Validates a wheel filename against PyPI platform tag rules
/// Extracts platform tags from wheel filename and validates each one
pub fn validate_wheel_filename_for_pypi(filename: &str) -> Result<()> {
    // Parse wheel filename to extract platform tags
    let platform_tags = extract_platform_tags_from_wheel_filename(filename)?;

    for platform_tag in platform_tags {
        if !is_platform_tag_allowed_by_pypi(&platform_tag) {
            bail!("Platform tag '{platform_tag}' in wheel '{filename}' is not allowed by PyPI");
        }
    }

    Ok(())
}

/// Extracts platform tags from a wheel filename.
///
/// Wheel filename format: `{name}-{version}-{python_tag}-{abi_tag}-{platform_tag}.whl`.
fn extract_platform_tags_from_wheel_filename(filename: &str) -> Result<Vec<String>> {
    let name_without_ext = filename
        .strip_suffix(".whl")
        .ok_or_else(|| anyhow!("Not a wheel file: {filename}"))?;

    let parts: Vec<&str> = name_without_ext.split('-').collect();

    if parts.len() < 5 {
        bail!("Invalid wheel filename format: {filename}");
    }

    // Platform tag is the last part, and can contain multiple tags separated by '.'
    let platform_part = parts[parts.len() - 1];
    let platform_tags: Vec<String> = platform_part.split('.').map(|s| s.to_string()).collect();

    Ok(platform_tags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_options::TargetTriple;
    use crate::target::Target;

    #[test]
    fn test_platform_tag_validation() {
        let tags = [
            // Simple platforms
            ("win32", true),
            ("win_amd64", true),
            ("any", true),
            // manylinux platforms
            ("manylinux2014_x86_64", true),
            ("manylinux_2_17_aarch64", true),
            ("manylinux_2_17_riscv64", false),
            // musllinux platforms
            ("musllinux_1_1_x86_64", true),
            ("musllinux_1_1_riscv64", false),
            // macOS platforms
            ("macosx_9_0_x86_64", false), // Invalid major version
            ("macosx_10_9_x86_64", true),
            ("macosx_11_0_arm64", true),
            // iOS platforms
            ("ios_14_0_arm64_iphoneos", true),
            ("ios_14_0_x86_64_iphonesimulator", true),
            ("ios_14_0_i686_iphoneos", false), // Unsupported architecture
            // Android platforms
            ("android_21_armeabi_v7a", true),
            ("android_21_arm64_v8a", true),
            ("android_21_x86", true),
            ("android_21_x86_64", true),
            ("android_21_mips", false), // Unsupported architecture
        ];

        for (platform_tag, expected) in tags {
            assert_eq!(
                is_platform_tag_allowed_by_pypi(platform_tag),
                expected,
                "{platform_tag}"
            );
        }
    }

    #[test]
    fn test_wheel_filename_parsing() {
        let wheel_filenames = [
            ("test-1.0.0-py3-none-win_amd64.whl", true),
            ("test-1.0.0-py3-none-manylinux2014_x86_64.whl", true),
            ("test-1.0.0-py3-none-any.whl", true),
            ("test-1.0.0-py3-none-linux_riscv64.whl", false),
        ];

        for (filename, should_succeed) in wheel_filenames {
            let result = validate_wheel_filename_for_pypi(filename);
            if should_succeed {
                assert!(result.is_ok(), "{filename}");
            } else {
                assert!(result.is_err(), "{filename}");
            }
        }
    }

    #[test]
    fn test_target_arch_validation() {
        let targets = [
            ("x86_64-pc-windows-msvc", true),
            ("aarch64-apple-darwin", true),
            ("x86_64-unknown-linux-gnu", true),
            ("aarch64-linux-android", true),
            ("armv7-linux-androideabi", true),
            ("riscv64gc-unknown-linux-gnu", false), // Unsupported
            ("x86_64-unknown-freebsd", false),      // Now unsupported (no lazy validation)
            ("powerpc64-unknown-linux-gnu", true),  // PPC64 on Linux is supported
            ("s390x-unknown-linux-gnu", true),      // s390x on Linux is supported
            ("wasm32-unknown-emscripten", false),   // Emscripten is unsupported
            ("i686-pc-windows-msvc", true),         // i686 Windows is supported
        ];

        for (triple, expected) in targets {
            let target =
                Target::from_target_triple(Some(&TargetTriple::Regular(triple.to_string())))
                    .unwrap();
            assert_eq!(is_arch_supported_by_pypi(&target), expected, "{triple}");
        }
    }
}
