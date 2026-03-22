use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Pre-generated SBOM data that can be reused across multiple wheel writes.
///
/// Since the Rust dependency graph is the same regardless of the target Python
/// interpreter, we generate the SBOM once per `BuildContext` build and reuse
/// the resulting bytes for every wheel.
pub struct SbomData {
    /// Generated Rust SBOM entries: (package_name, json_bytes).
    pub rust_sboms: Vec<(String, Vec<u8>)>,
}

/// Validate and resolve an SBOM include path.
///
/// Absolute paths are used as-is (after canonicalization).
/// Relative paths are resolved against the project root and must stay within it.
///
/// Returns the canonicalized path on success.
pub(crate) fn resolve_sbom_include(path: &Path, project_root: &Path) -> Result<PathBuf> {
    let is_absolute = path.is_absolute();
    let resolved_path = if is_absolute {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };

    let resolved_path = resolved_path.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize SBOM include path '{}'",
            resolved_path.display()
        )
    })?;

    // Only enforce the project-root constraint for relative paths
    // to prevent directory traversal (e.g. "../../etc/passwd").
    // Absolute paths are intentionally allowed to reference files
    // outside the project root.
    if !is_absolute && !resolved_path.starts_with(project_root) {
        anyhow::bail!(
            "SBOM include path '{}' escapes the project root '{}'",
            resolved_path.display(),
            project_root.display()
        );
    }

    Ok(resolved_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err as fs;
    use tempfile::tempdir;

    #[test]
    fn test_reject_path_escaping_project_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let result = resolve_sbom_include(Path::new("../../etc/passwd"), &root);
        assert!(result.is_err());
    }

    #[test]
    fn test_accept_valid_relative_path() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let sbom_file = root.join("my_sbom.json");
        fs::write(&sbom_file, "{}").unwrap();
        let result = resolve_sbom_include(Path::new("my_sbom.json"), &root).unwrap();
        assert_eq!(result, sbom_file);
    }

    #[test]
    fn test_accept_nested_path() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let nested = root.join("sboms/vendor");
        fs::create_dir_all(&nested).unwrap();
        let sbom_file = nested.join("report.json");
        fs::write(&sbom_file, "{}").unwrap();
        let result = resolve_sbom_include(Path::new("sboms/vendor/report.json"), &root).unwrap();
        assert_eq!(result, sbom_file);
    }

    #[test]
    fn test_accept_absolute_path_outside_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let outside = tempdir().unwrap();
        let outside_file = outside.path().join("external.json");
        fs::write(&outside_file, "{}").unwrap();
        let result = resolve_sbom_include(&outside_file, &root).unwrap();
        assert_eq!(result, outside_file.canonicalize().unwrap());
    }
    #[test]
    fn test_reject_nonexistent_path() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let result = resolve_sbom_include(Path::new("does_not_exist.json"), &root);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to canonicalize")
        );
    }
}
