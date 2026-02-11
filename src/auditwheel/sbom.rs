//! Generate a CycloneDX SBOM for shared libraries grafted by auditwheel repair.
//!
//! When maturin copies external shared libraries into the wheel (the "repair"
//! step), this module records which OS packages provided those libraries and
//! produces a [CycloneDX 1.4](https://cyclonedx.org/) SBOM that is stored at
//! `<dist-info>/sboms/auditwheel.cdx.json` inside the wheel.
//!
//! # Differences from Python auditwheel
//!
//! * **Tool identity** – Python auditwheel records `"auditwheel"` in
//!   `metadata.tools`.  We record `"maturin"` instead.
//!
//! * **PURL `file_name` qualifier omitted** – Python auditwheel includes
//!   `?file_name=<wheel_filename>` in the wheel's PURL.  In maturin the SBOM
//!   is generated *during* wheel writing before the final filename is known, so
//!   we omit it.  The PURL is still valid (`pkg:pypi/<name>@<version>`).
//!
//! * **Cross-compilation sysroot support** – The sysroot is forwarded to
//!   [`whichprovides`](super::whichprovides) so it can strip sysroot prefixes
//!   and read the target's `/etc/os-release` when available.

use once_cell::sync::Lazy;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::whichprovides::{self, purl_encode};

static NAME_NORMALIZE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[-_.]+").unwrap());

/// Create a CycloneDX SBOM for shared libraries grafted during auditwheel repair.
///
/// Returns `None` if `grafted_lib_paths` is empty or no package-manager
/// information could be determined for any of them.
pub fn create_auditwheel_sbom(
    wheel_name: &str,
    wheel_version: &str,
    grafted_lib_paths: &[PathBuf],
    sysroot: &Path,
) -> Option<Vec<u8>> {
    if grafted_lib_paths.is_empty() {
        return None;
    }

    let packages = whichprovides::whichprovides(grafted_lib_paths, sysroot);
    if packages.is_empty() {
        return None;
    }

    // PEP 503 name normalisation: lowercase, collapse [-_.]+  to `-`.
    let name = NAME_NORMALIZE_RE
        .replace_all(wheel_name, "-")
        .to_lowercase();
    let wheel_purl = format!(
        "pkg:pypi/{}@{}",
        purl_encode(&name),
        purl_encode(wheel_version),
    );

    let root = serde_json::json!({
        "type": "library",
        "bom-ref": &wheel_purl,
        "name": &name,
        "version": wheel_version,
        "purl": &wheel_purl,
    });

    let mut components = vec![root.clone()];
    let mut depends_on = Vec::new();
    let mut dep_refs = Vec::new();

    // Sort by filepath for deterministic SBOM output (HashMap iteration
    // order is non-deterministic).
    let mut sorted_packages: Vec<_> = packages.iter().collect();
    sorted_packages.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (filepath, provided_by) in sorted_packages {
        // Use a hash of the filepath to disambiguate components from the same
        // package (matching Python auditwheel's approach).
        let hash = format!(
            "{:x}",
            Sha256::digest(filepath.to_string_lossy().as_bytes())
        );
        let bom_ref = format!("{}#{hash}", provided_by.purl());

        components.push(serde_json::json!({
            "type": "library",
            "bom-ref": &bom_ref,
            "name": &provided_by.package_name,
            "version": &provided_by.package_version,
            "purl": provided_by.purl(),
        }));
        depends_on.push(bom_ref.clone());
        dep_refs.push(serde_json::json!({"ref": bom_ref}));
    }

    let mut dependencies = vec![serde_json::json!({
        "ref": &wheel_purl,
        "dependsOn": depends_on,
    })];
    dependencies.extend(dep_refs);

    let sbom = serde_json::json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.4",
        "version": 1,
        "metadata": {
            "component": root,
            "tools": [{"name": "maturin", "version": env!("CARGO_PKG_VERSION")}],
        },
        "components": components,
        "dependencies": dependencies,
    });

    serde_json::to_vec_pretty(&sbom).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name_normalisation() {
        let norm = |s| NAME_NORMALIZE_RE.replace_all(s, "-").to_lowercase();
        assert_eq!(norm("My_Package"), "my-package");
        assert_eq!(norm("foo.bar__baz"), "foo-bar-baz");
        assert_eq!(norm("simple"), "simple");
    }

    #[test]
    fn test_returns_none_for_empty_paths() {
        assert!(create_auditwheel_sbom("pkg", "1.0", &[], Path::new("/")).is_none());
    }
}
