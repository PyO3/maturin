//! Generate a CycloneDX SBOM for shared libraries grafted by auditwheel repair.
//!
//! When maturin copies external shared libraries into the wheel (the "repair"
//! step), this module records which OS packages provided those libraries and
//! produces a [CycloneDX 1.4](https://cyclonedx.org/) SBOM that is stored at
//! `<dist-info>/sboms/auditwheel.cdx.json` inside the wheel.
//!
//! # Differences from Python auditwheel
//!
//! * **Tool identity** – Python auditwheel records `{"name": "auditwheel",
//!   "version": "<auditwheel_version>"}` in `metadata.tools`.  We record
//!   `{"name": "maturin", "version": "<maturin_version>"}` instead, since
//!   maturin is the tool performing the repair.
//!
//! * **PURL `file_name` qualifier omitted** – Python auditwheel includes
//!   `?file_name=<wheel_filename>` in the wheel's PURL because the final
//!   wheel filename is known at SBOM creation time (repair happens after the
//!   wheel is built).  In maturin the repair and SBOM generation happen
//!   *during* wheel writing, before the final filename is determined, so we
//!   omit the `file_name` qualifier.  The PURL is still valid and unique
//!   (`pkg:pypi/<name>@<version>`).
//!
//! * **Cross-compilation sysroot support** – Python auditwheel always runs
//!   natively, so library paths are direct host paths.  Maturin can cross-
//!   compile with a foreign sysroot; we pass the sysroot through to
//!   [`whichprovides`](super::whichprovides) so it can strip sysroot prefixes
//!   and read the target's `/etc/os-release` when available.
//!
//! * **Spec version** – We target CycloneDX 1.4, matching Python auditwheel.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::whichprovides::{self, ProvidedBy, purl_encode};

/// Create a CycloneDX SBOM for shared libraries grafted during auditwheel repair.
///
/// `wheel_name` and `wheel_version` are the Python distribution name and
/// version (used to build the root component's PURL).
///
/// `grafted_lib_paths` are the *original* (pre-copy) filesystem paths of the
/// shared libraries that were grafted into the wheel.
///
/// `sysroot` is the root directory used by `lddtree` for library resolution
/// (typically `/` for native builds).  It is forwarded to [`whichprovides`] to
/// handle cross-compilation sysroot prefix stripping.
///
/// Returns `None` if no libraries were provided or if no package-manager
/// information could be determined for any of them (matching Python
/// auditwheel's behavior of silently omitting the SBOM in that case).
pub fn create_auditwheel_sbom(
    wheel_name: &str,
    wheel_version: &str,
    grafted_lib_paths: &[PathBuf],
    sysroot: &Path,
) -> Option<Vec<u8>> {
    if grafted_lib_paths.is_empty() {
        return None;
    }

    let sbom_packages: HashMap<PathBuf, ProvidedBy> =
        whichprovides::whichprovides(grafted_lib_paths, sysroot);
    if sbom_packages.is_empty() {
        return None;
    }

    // Normalise wheel name for PURL: PEP 503 normalisation (lowercased,
    // runs of [-_.] replaced with a single hyphen).
    let normalised_name = purl_normalise_name(wheel_name);
    let wheel_purl = format!(
        "pkg:pypi/{}@{}",
        purl_encode(&normalised_name),
        purl_encode(wheel_version),
    );

    let maturin_version = env!("CARGO_PKG_VERSION");

    // Build the root component (the wheel itself).
    let root_component = serde_json::json!({
        "type": "library",
        "bom-ref": &wheel_purl,
        "name": &normalised_name,
        "version": wheel_version,
        "purl": &wheel_purl,
    });

    let mut components = vec![root_component.clone()];
    let mut depends_on: Vec<serde_json::Value> = Vec::new();
    let mut dep_entries: Vec<serde_json::Value> = Vec::new();

    for (filepath, provided_by) in &sbom_packages {
        let filepath_str = filepath.to_string_lossy();
        let hash = sha256_hex(filepath_str.as_bytes());
        let bom_ref = format!("{}#{}", provided_by.purl(), hash);

        components.push(serde_json::json!({
            "type": "library",
            "bom-ref": &bom_ref,
            "name": &provided_by.package_name,
            "version": &provided_by.package_version,
            "purl": provided_by.purl(),
        }));

        depends_on.push(serde_json::Value::String(bom_ref.clone()));
        dep_entries.push(serde_json::json!({"ref": bom_ref}));
    }

    let mut dependencies = vec![serde_json::json!({
        "ref": &wheel_purl,
        "dependsOn": depends_on,
    })];
    dependencies.extend(dep_entries);

    let sbom = serde_json::json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.4",
        "version": 1,
        "metadata": {
            "component": root_component,
            "tools": [
                {"name": "maturin", "version": maturin_version},
            ],
        },
        "components": components,
        "dependencies": dependencies,
    });

    // Serialise to pretty-printed JSON for readability.
    serde_json::to_vec_pretty(&sbom).ok()
}

/// PEP 503 name normalisation: lowercase, collapse `[-_.]+` to `-`.
fn purl_normalise_name(name: &str) -> String {
    let re = regex::Regex::new(r"[-_.]+").unwrap();
    re.replace_all(name, "-").to_lowercase()
}

/// SHA-256 hex digest of arbitrary bytes.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_purl_normalise_name() {
        assert_eq!(purl_normalise_name("My_Package"), "my-package");
        assert_eq!(purl_normalise_name("foo.bar__baz"), "foo-bar-baz");
        assert_eq!(purl_normalise_name("simple"), "simple");
    }

    #[test]
    fn test_returns_none_for_empty_paths() {
        assert!(create_auditwheel_sbom("pkg", "1.0", &[], Path::new("/")).is_none());
    }

    // NOTE: Testing the full SBOM generation with real OS package lookups
    // requires a Linux environment.  The `whichprovides` module has its own
    // unit tests for parsing logic; end-to-end tests belong in CI containers.
}
