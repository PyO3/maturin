//! Identify which OS package provides a shared library file.
//!
//! This is a Rust port of Python auditwheel's `_vendor/whichprovides` module.
//! It queries the system package manager (dpkg, rpm, apk) to discover which
//! OS-level package "owns" a given file path, then returns a typed record
//! containing the package name, version, distro, and a Package URL (PURL).
//!
//! # Differences from Python auditwheel
//!
//! * **No `apt-file` provider** – Python auditwheel includes an `AptFileProvider`
//!   as a slow fallback for Debian-family systems. We omit it because `dpkg -S`
//!   already covers all *installed* packages (which is the relevant set during
//!   `auditwheel repair` — the library must be present on disk to be grafted).
//!   This avoids shelling out to `apt-file` which requires a separate index
//!   update and is significantly slower.
//!
//! * **No provider priority ordering** – In Python auditwheel every provider has
//!   a `_resolve_order` field so that expensive providers (apt-file) run last.
//!   Since we only have the three fast, mutually-exclusive providers (exactly one
//!   of dpkg / rpm / apk will be available on a given system), ordering is
//!   irrelevant and we simply try them in sequence.
//!
//! * **Result caching** – Python auditwheel caches the `which` lookup for
//!   provider binaries across calls.  We perform the detection once per
//!   `whichprovides()` invocation, which is called at most once per wheel build,
//!   so caching is unnecessary.
//!
//! * **Cross-compilation sysroot awareness** – Python auditwheel always runs
//!   natively inside a manylinux/musllinux container, so it queries the host's
//!   package manager directly.  Maturin can cross-compile from a host machine
//!   using a foreign sysroot (e.g. `aarch64-linux-gnu` packages installed via
//!   `dpkg --add-architecture`).  When a sysroot is provided:
//!
//!   - Library paths are first queried as-is (they may live in a dpkg multiarch
//!     directory like `/usr/aarch64-linux-gnu/lib/` which dpkg tracks natively).
//!   - If that fails and the path starts with the sysroot prefix, the prefix is
//!     stripped and the host-relative path (e.g. `/usr/lib/...`) is tried instead.
//!   - The distro ID is read from `<sysroot>/etc/os-release` when the sysroot
//!     differs from `/`, falling back to the host's `/etc/os-release`.
//!
//!   This means SBOM generation works in the common cross-compilation scenarios:
//!   (a) manylinux/musllinux Docker containers (sysroot = `/`),
//!   (b) Debian/Ubuntu multiarch cross-compilation with `dpkg --add-architecture`,
//!   (c) standalone sysroots where the libraries are not tracked by any host
//!   package manager — in this case no SBOM is produced (graceful no-op).

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use once_cell::sync::Lazy;
use regex::Regex;

/// Regex for parsing `ID=...` lines from `/etc/os-release`.
static OS_RELEASE_ID_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^ID=(?:"([^"]*)"|(.*))\s*$"#).unwrap());

/// Regex for parsing `dpkg -S` output: "package:arch: /path".
static DPKG_SEARCH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([^:]+):").unwrap());

/// Regex for parsing `dpkg -s` version output.
static DPKG_VERSION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^Version:\s*(\S+)").unwrap());

/// Regex for parsing `apk info --who-owns` output.
static APK_WHO_OWNS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r" is owned by ([^\s\-]+)-([^\s]+)$").unwrap());

/// Information about the OS package that provides a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvidedBy {
    /// Package type identifier (e.g. "deb", "rpm", "apk").
    pub package_type: String,
    /// Package name as reported by the package manager.
    pub package_name: String,
    /// Package version string.
    pub package_version: String,
    /// Distro identifier from `/etc/os-release` `ID=` (e.g. "ubuntu", "alpine").
    pub distro: Option<String>,
}

impl ProvidedBy {
    /// Returns a [Package URL (PURL)](https://github.com/package-url/purl-spec)
    /// for this package.
    ///
    /// Format: `pkg:<type>[/<distro>]/<name>@<version>`
    pub fn purl(&self) -> String {
        let mut parts = format!("pkg:{}/", self.package_type);
        if let Some(distro) = &self.distro {
            parts.push_str(&purl_encode(distro));
            parts.push('/');
        }
        parts.push_str(&purl_encode(&self.package_name));
        parts.push('@');
        parts.push_str(&purl_encode(&self.package_version));
        parts
    }
}

impl fmt::Display for ProvidedBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.purl())
    }
}

/// Percent-encode a value for use in a PURL component.
///
/// PURL uses standard percent-encoding but only for characters that are not
/// unreserved (RFC 3986).  We take the simple approach of encoding everything
/// that is *not* alphanumeric, `-`, `.`, or `_`.
pub(super) fn purl_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.' || byte == b'_' {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Read the distro `ID` from an `os-release` file.
///
/// Returns `None` if the file cannot be read or does not contain an `ID=` line.
fn read_distro_id_from(path: &Path) -> Option<String> {
    let content = fs_err::read_to_string(path).ok()?;
    let caps = OS_RELEASE_ID_RE.captures(&content)?;
    caps.get(1)
        .or_else(|| caps.get(2))
        .map(|m| m.as_str().to_string())
}

/// Read the distro `ID`, trying the sysroot first then falling back to host.
fn read_distro_id(sysroot: &Path) -> Option<String> {
    if sysroot != Path::new("/") {
        // Try sysroot's os-release first (reflects the target environment).
        if let Some(id) = read_distro_id_from(&sysroot.join("etc/os-release")) {
            return Some(id);
        }
    }
    // Fall back to host os-release.
    read_distro_id_from(Path::new("/etc/os-release"))
}

/// Check whether an executable exists on `$PATH` by attempting to run it.
fn has_bin(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

// ---------------------------------------------------------------------------
// Provider implementations
// ---------------------------------------------------------------------------

/// Try `dpkg -S <path>` then `dpkg -s <package>` (Debian/Ubuntu).
fn dpkg_whichprovides(filepath: &str, distro: &str) -> Option<ProvidedBy> {
    // dpkg -S /usr/lib/x86_64-linux-gnu/libz.so.1
    //   => "zlib1g:amd64: /usr/lib/x86_64-linux-gnu/libz.so.1"
    let output = Command::new("dpkg").args(["-S", filepath]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let package_name = DPKG_SEARCH_RE
        .captures(&stdout)?
        .get(1)?
        .as_str()
        .to_string();

    // dpkg -s zlib1g  => "Version: 1:1.2.11.dfsg-2ubuntu9"
    let output = Command::new("dpkg")
        .args(["-s", &package_name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = DPKG_VERSION_RE
        .captures(&stdout)?
        .get(1)?
        .as_str()
        .to_string();

    Some(ProvidedBy {
        package_type: "deb".to_string(),
        package_name,
        package_version: version,
        distro: Some(distro.to_string()),
    })
}

/// Try `rpm -qf` (RHEL/CentOS/Fedora/SUSE).
fn rpm_whichprovides(filepath: &str, distro: &str) -> Option<ProvidedBy> {
    // rpm -qf --queryformat "%{NAME} %{VERSION} %{RELEASE} %{ARCH}" /usr/lib64/libz.so.1
    //   => "zlib 1.2.11 31.el9 x86_64"
    let output = Command::new("rpm")
        .args([
            "-qf",
            "--queryformat",
            "%{NAME} %{VERSION} %{RELEASE} %{ARCH}",
            filepath,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.trim().splitn(4, ' ').collect();
    if parts.len() < 3 {
        return None;
    }
    let package_name = parts[0].to_string();
    let package_version = format!("{}-{}", parts[1], parts[2]);

    Some(ProvidedBy {
        package_type: "rpm".to_string(),
        package_name,
        package_version,
        distro: Some(distro.to_string()),
    })
}

/// Try `apk info --who-owns` (Alpine).
fn apk_whichprovides(filepath: &str, distro: &str) -> Option<ProvidedBy> {
    // apk info --who-owns /lib/libz.so.1
    //   => "/lib/libz.so.1 is owned by zlib-1.3.1-r2"
    let output = Command::new("apk")
        .args(["info", "--who-owns", filepath])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let caps = APK_WHO_OWNS_RE.captures(stdout.trim())?;
    let package_name = caps.get(1)?.as_str().to_string();
    let package_version = caps.get(2)?.as_str().to_string();

    Some(ProvidedBy {
        package_type: "apk".to_string(),
        package_name,
        package_version,
        distro: Some(distro.to_string()),
    })
}

/// The provider function signature: `(filepath, distro) -> Option<ProvidedBy>`.
type ProviderFn = fn(&str, &str) -> Option<ProvidedBy>;

/// Detect which package-manager provider is available on the host.
///
/// Returns the provider function and the distro ID, or `None` if no supported
/// package manager is found.
fn detect_provider(sysroot: &Path) -> Option<(ProviderFn, String)> {
    let distro = read_distro_id(sysroot)?;

    // Try providers in order: dpkg (Debian/Ubuntu), rpm (RHEL/Fedora/SUSE), apk (Alpine).
    // On any given Linux system, exactly one of these should be the "native" manager.
    if has_bin("dpkg") {
        return Some((dpkg_whichprovides as ProviderFn, distro));
    }
    if has_bin("rpm") {
        return Some((rpm_whichprovides as ProviderFn, distro));
    }
    if has_bin("apk") {
        return Some((apk_whichprovides as ProviderFn, distro));
    }

    None
}

/// Identify which OS package provides each of the given file paths.
///
/// `sysroot` is the root directory used by `lddtree` for library resolution
/// (typically `/` for native builds or a cross-compiler sysroot path).  When
/// the sysroot is not `/`, library paths that start with the sysroot prefix are
/// also tried with the prefix stripped — this handles the common case where
/// `lddtree` resolves libraries under the sysroot but `dpkg`/`rpm`/`apk` on
/// the host tracks them by their host-relative path.
///
/// Returns a map from the *original* path to the [`ProvidedBy`] result.  Paths
/// whose owning package cannot be determined are silently omitted.
pub fn whichprovides(filepaths: &[PathBuf], sysroot: &Path) -> HashMap<PathBuf, ProvidedBy> {
    let mut results = HashMap::new();

    let (provider_fn, distro) = match detect_provider(sysroot) {
        Some(v) => v,
        None => return results,
    };

    // Canonicalize sysroot once for prefix-stripping.
    let canon_sysroot = sysroot
        .canonicalize()
        .unwrap_or_else(|_| sysroot.to_path_buf());

    for filepath in filepaths {
        // Resolve symlinks so the package manager can match the canonical path.
        let resolved = filepath.canonicalize().unwrap_or_else(|_| filepath.clone());
        let resolved_str = resolved.to_string_lossy();

        // First, try the path as-is (works for native builds, Docker containers,
        // and Debian multiarch cross-packages whose files dpkg tracks at their
        // full sysroot path).
        if let Some(provided_by) = provider_fn(&resolved_str, &distro) {
            results.insert(filepath.clone(), provided_by);
            continue;
        }

        // If the sysroot is not `/` and the path starts with it, strip the
        // sysroot prefix and retry.  This handles cross-compilation sysroots
        // where the host's package manager knows the library by its
        // target-relative path (e.g. `/usr/lib/libz.so.1` rather than
        // `/usr/aarch64-linux-gnu/usr/lib/libz.so.1`).
        if canon_sysroot != Path::new("/")
            && let Ok(rel) = resolved.strip_prefix(&canon_sysroot)
        {
            let host_path = Path::new("/").join(rel);
            let host_str = host_path.to_string_lossy();
            if let Some(provided_by) = provider_fn(&host_str, &distro) {
                results.insert(filepath.clone(), provided_by);
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_purl_encode_simple() {
        assert_eq!(purl_encode("zlib1g"), "zlib1g");
    }

    #[test]
    fn test_purl_encode_special() {
        assert_eq!(purl_encode("foo bar"), "foo%20bar");
        assert_eq!(purl_encode("a/b"), "a%2Fb");
    }

    #[test]
    fn test_provided_by_purl_with_distro() {
        let p = ProvidedBy {
            package_type: "deb".to_string(),
            package_name: "zlib1g".to_string(),
            package_version: "1:1.2.11".to_string(),
            distro: Some("ubuntu".to_string()),
        };
        assert_eq!(p.purl(), "pkg:deb/ubuntu/zlib1g@1%3A1.2.11");
    }

    #[test]
    fn test_provided_by_purl_without_distro() {
        let p = ProvidedBy {
            package_type: "rpm".to_string(),
            package_name: "zlib".to_string(),
            package_version: "1.2.11-31.el9".to_string(),
            distro: None,
        };
        assert_eq!(p.purl(), "pkg:rpm/zlib@1.2.11-31.el9");
    }

    #[test]
    fn test_provided_by_display() {
        let p = ProvidedBy {
            package_type: "apk".to_string(),
            package_name: "zlib".to_string(),
            package_version: "1.3.1-r2".to_string(),
            distro: Some("alpine".to_string()),
        };
        assert_eq!(format!("{p}"), "pkg:apk/alpine/zlib@1.3.1-r2");
    }

    #[test]
    fn test_whichprovides_empty_input() {
        let result = whichprovides(&[], Path::new("/"));
        assert!(result.is_empty());
    }

    // NOTE: Integration tests that actually invoke dpkg/rpm/apk require a
    // Linux environment with the relevant package manager installed.  Those
    // are best tested in CI containers.
}
