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
//!   as a slow fallback for Debian-family systems.  We omit it because `dpkg -S`
//!   already covers all *installed* packages (which is the relevant set during
//!   `auditwheel repair` — the library must be present on disk to be grafted).
//!
//! * **Cross-compilation sysroot awareness** – Python auditwheel always runs
//!   natively inside a manylinux/musllinux container.  Maturin can cross-compile
//!   from a host machine using a foreign sysroot.  When a sysroot is provided:
//!
//!   - Library paths are first queried as-is (they may live in a dpkg multiarch
//!     directory like `/usr/aarch64-linux-gnu/lib/` which dpkg tracks natively).
//!   - If that fails and the path starts with the sysroot prefix, the prefix is
//!     stripped and the host-relative path (e.g. `/usr/lib/...`) is tried instead.
//!   - The distro ID is read from `<sysroot>/etc/os-release` when the sysroot
//!     differs from `/`, falling back to the host's `/etc/os-release`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use once_cell::sync::Lazy;
use regex::Regex;

static OS_RELEASE_ID_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^ID=(?:"([^"]*)"|(.*))\s*$"#).unwrap());
static DPKG_SEARCH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([^:]+):").unwrap());
static DPKG_VERSION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^Version:\s*(\S+)").unwrap());
static APK_WHO_OWNS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r" is owned by (.+)-(\d[^\s]*)$").unwrap());

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
    /// for this package: `pkg:<type>[/<distro>]/<name>@<version>`.
    pub fn purl(&self) -> String {
        let mut s = format!("pkg:{}/", self.package_type);
        if let Some(distro) = &self.distro {
            s.push_str(&purl_encode(distro));
            s.push('/');
        }
        s.push_str(&purl_encode(&self.package_name));
        s.push('@');
        s.push_str(&purl_encode(&self.package_version));
        s
    }
}

/// Percent-encode a value for use in a PURL component.
pub(super) fn purl_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Provider implementations
// ---------------------------------------------------------------------------

/// Try `dpkg -S <path>` then `dpkg -s <package>` (Debian/Ubuntu).
fn dpkg_provides(filepath: &str, distro: &str) -> Option<ProvidedBy> {
    let out = Command::new("dpkg").args(["-S", filepath]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let package_name = DPKG_SEARCH_RE.captures(&stdout)?.get(1)?.as_str();

    let out = Command::new("dpkg")
        .args(["-s", package_name])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let version = DPKG_VERSION_RE.captures(&stdout)?.get(1)?.as_str();

    Some(ProvidedBy {
        package_type: "deb".into(),
        package_name: package_name.into(),
        package_version: version.into(),
        distro: Some(distro.into()),
    })
}

/// Try `rpm -qf` (RHEL/CentOS/Fedora/SUSE).
fn rpm_provides(filepath: &str, distro: &str) -> Option<ProvidedBy> {
    let out = Command::new("rpm")
        .args([
            "-qf",
            "--queryformat",
            "%{NAME} %{VERSION} %{RELEASE}",
            filepath,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut parts = stdout.trim().splitn(3, ' ');
    let name = parts.next()?;
    let version = parts.next()?;
    let release = parts.next()?;

    Some(ProvidedBy {
        package_type: "rpm".into(),
        package_name: name.into(),
        package_version: format!("{version}-{release}"),
        distro: Some(distro.into()),
    })
}

/// Try `apk info --who-owns` (Alpine).
fn apk_provides(filepath: &str, distro: &str) -> Option<ProvidedBy> {
    let out = Command::new("apk")
        .args(["info", "--who-owns", filepath])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let caps = APK_WHO_OWNS_RE.captures(stdout.trim())?;

    Some(ProvidedBy {
        package_type: "apk".into(),
        package_name: caps[1].into(),
        package_version: caps[2].into(),
        distro: Some(distro.into()),
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Identify which OS package provides each of the given file paths.
///
/// `sysroot` is the root directory used by `lddtree` for library resolution
/// (typically `/` for native builds or a cross-compiler sysroot path).  When
/// the sysroot is not `/`, library paths that start with the sysroot prefix are
/// also tried with the prefix stripped.
///
/// Paths whose owning package cannot be determined are silently omitted.
pub fn whichprovides(filepaths: &[PathBuf], sysroot: &Path) -> HashMap<PathBuf, ProvidedBy> {
    let mut results = HashMap::new();

    // Detect provider: exactly one of dpkg/rpm/apk is the native manager.
    let distro = read_distro_id(sysroot);
    let distro = match distro.as_deref() {
        Some(d) => d,
        None => return results,
    };

    let provider: fn(&str, &str) -> Option<ProvidedBy> = if which::which("dpkg").is_ok() {
        dpkg_provides
    } else if which::which("rpm").is_ok() {
        rpm_provides
    } else if which::which("apk").is_ok() {
        apk_provides
    } else {
        return results;
    };

    let canon_sysroot = sysroot.canonicalize().unwrap_or_else(|_| sysroot.into());

    for filepath in filepaths {
        let resolved = filepath.canonicalize().unwrap_or_else(|_| filepath.clone());
        let resolved_str = resolved.to_string_lossy();

        // Try the path as-is first.
        if let Some(provided_by) = provider(&resolved_str, distro) {
            results.insert(filepath.clone(), provided_by);
            continue;
        }

        // For non-root sysroots, strip the prefix and retry with the
        // host-relative path.
        if canon_sysroot != Path::new("/")
            && let Ok(rel) = resolved.strip_prefix(&canon_sysroot)
        {
            let host_path = Path::new("/").join(rel);
            if let Some(provided_by) = provider(&host_path.to_string_lossy(), distro) {
                results.insert(filepath.clone(), provided_by);
            }
        }
    }

    results
}

/// Read the distro `ID`, trying the sysroot first then falling back to host.
fn read_distro_id(sysroot: &Path) -> Option<String> {
    if sysroot != Path::new("/")
        && let Some(id) = read_os_release_id(&sysroot.join("etc/os-release"))
    {
        return Some(id);
    }
    read_os_release_id(Path::new("/etc/os-release"))
}

fn read_os_release_id(path: &Path) -> Option<String> {
    let content = fs_err::read_to_string(path).ok()?;
    let caps = OS_RELEASE_ID_RE.captures(&content)?;
    caps.get(1)
        .or_else(|| caps.get(2))
        .map(|m| m.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_purl_encode() {
        assert_eq!(purl_encode("zlib1g"), "zlib1g");
        assert_eq!(purl_encode("foo bar"), "foo%20bar");
        assert_eq!(purl_encode("a/b"), "a%2Fb");
    }

    #[test]
    fn test_purl_with_distro() {
        let p = ProvidedBy {
            package_type: "deb".into(),
            package_name: "zlib1g".into(),
            package_version: "1:1.2.11".into(),
            distro: Some("ubuntu".into()),
        };
        assert_eq!(p.purl(), "pkg:deb/ubuntu/zlib1g@1%3A1.2.11");
    }

    #[test]
    fn test_purl_without_distro() {
        let p = ProvidedBy {
            package_type: "rpm".into(),
            package_name: "zlib".into(),
            package_version: "1.2.11-31.el9".into(),
            distro: None,
        };
        assert_eq!(p.purl(), "pkg:rpm/zlib@1.2.11-31.el9");
    }

    #[test]
    fn test_whichprovides_empty_input() {
        assert!(whichprovides(&[], Path::new("/")).is_empty());
    }
}
