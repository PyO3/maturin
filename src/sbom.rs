use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[cfg(feature = "sbom")]
use cargo_cyclonedx::config::SbomConfig as CyclonedxConfig;
#[cfg(feature = "sbom")]
use cargo_cyclonedx::generator::SbomGenerator;

use crate::BuildContext;
use crate::module_writer::ModuleWriter;

/// Pre-generated SBOM data that can be reused across multiple wheel writes.
///
/// Since the Rust dependency graph is the same regardless of the target Python
/// interpreter, we generate the SBOM once per `BuildContext` build and reuse
/// the resulting bytes for every wheel.
pub struct SbomData {
    /// Generated Rust SBOM entries: (package_name, json_bytes).
    pub rust_sboms: Vec<(String, Vec<u8>)>,
}

/// Generate Rust SBOMs once from the build context.
///
/// When the `sbom` feature is enabled, Rust SBOM generation is on by default.
/// It can be explicitly disabled via `[tool.maturin.sbom] rust = false`.
/// Returns `None` when the `sbom` feature is not compiled in.
pub fn generate_sbom_data(context: &BuildContext) -> Result<Option<SbomData>> {
    let sbom_config = context.sbom.as_ref();

    // Check if Rust SBOM generation is explicitly disabled
    let rust_sbom_enabled = sbom_config.and_then(|c| c.rust).unwrap_or(true);

    #[cfg(feature = "sbom")]
    {
        if !rust_sbom_enabled {
            return Ok(Some(SbomData {
                rust_sboms: Vec::new(),
            }));
        }

        let config = CyclonedxConfig {
            target: Some(cargo_cyclonedx::config::Target::AllTargets),
            ..CyclonedxConfig::empty_config()
        };
        // cargo-cyclonedx depends on cargo_metadata 0.18, while maturin uses
        // cargo_metadata 0.23. The Metadata structs are incompatible at the
        // type level but share the same JSON representation, so we bridge
        // them via a serde round-trip.
        let json = serde_json::to_value(&context.cargo_metadata)?;
        let metadata = serde_json::from_value(json)
            .context("Failed to convert cargo metadata for SBOM generation")?;
        let sboms = SbomGenerator::create_sboms(metadata, &config)
            .map_err(|e| anyhow::anyhow!("Failed to generate Rust SBOM: {}", e))?;

        let mut rust_sboms = Vec::new();
        for sbom in sboms {
            // Only keep the SBOM for the crate being built into a wheel.
            // Each member's SBOM already contains the full transitive
            // dependency graph, so filtering is safe.
            if sbom.package_name != context.crate_name {
                continue;
            }
            let mut buf = Vec::new();
            sbom.bom
                .output_as_json_v1_5(&mut buf)
                .map_err(|e| anyhow::anyhow!("Failed to serialize SBOM: {}", e))?;
            rust_sboms.push((sbom.package_name, buf));
        }

        Ok(Some(SbomData { rust_sboms }))
    }

    #[cfg(not(feature = "sbom"))]
    {
        let _ = rust_sbom_enabled;
        Ok(None)
    }
}

/// Validate and resolve an SBOM include path, ensuring it stays within the project root.
///
/// Returns the canonicalized path on success.
fn resolve_sbom_include(path: &Path, project_root: &Path) -> Result<PathBuf> {
    let resolved_path = if path.is_absolute() {
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

    if !resolved_path.starts_with(project_root) {
        anyhow::bail!(
            "SBOM include path '{}' escapes the project root '{}'",
            resolved_path.display(),
            project_root.display()
        );
    }

    Ok(resolved_path)
}

/// Writes SBOMs into the wheel via the given writer.
///
/// When the `sbom` feature is enabled, a Rust SBOM is included by default
/// unless explicitly disabled via `[tool.maturin.sbom] rust = false`.
/// Additional SBOM files can be included via `[tool.maturin.sbom] include = [...]`.
///
/// When the `sbom` feature is not compiled in, only user-provided `include`
/// files are written (if configured).
pub fn write_sboms(
    context: &BuildContext,
    sbom_data: Option<&SbomData>,
    writer: &mut impl ModuleWriter,
    dist_info_dir: &Path,
) -> Result<()> {
    let sbom_config = context.sbom.as_ref();

    // 1. Write pre-generated Rust SBOMs
    if let Some(data) = sbom_data {
        for (package_name, json_bytes) in &data.rust_sboms {
            let target = dist_info_dir.join(format!("sboms/{package_name}.cyclonedx.json"));
            writer.add_bytes(&target, None, json_bytes.clone(), false)?;
        }
    }

    // 2. Include additional SBOM files (only when explicitly configured)
    if let Some(include) = sbom_config.and_then(|c| c.include.as_ref()) {
        // Canonicalize project root once and enforce all includes stay within it.
        let project_root = context
            .project_layout
            .project_root
            .canonicalize()
            .context("Failed to canonicalize project root for SBOM includes")?;

        let mut seen_filenames = HashSet::new();
        for path in include {
            let resolved_path = resolve_sbom_include(path, &project_root)?;

            let filename = resolved_path.file_name().context("Invalid SBOM path")?;
            if !seen_filenames.insert(filename.to_os_string()) {
                anyhow::bail!(
                    "Duplicate SBOM filename '{}' from include path '{}'. \
                     Multiple includes must have unique filenames.",
                    filename.to_string_lossy(),
                    path.display()
                );
            }
            let target = dist_info_dir.join("sboms").join(filename);
            writer.add_file(&target, &resolved_path, false)?;
        }
    }

    Ok(())
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
    fn test_reject_absolute_path_outside_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let outside = tempdir().unwrap();
        let outside_file = outside.path().join("evil.json");
        fs::write(&outside_file, "{}").unwrap();
        let result = resolve_sbom_include(&outside_file, &root);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("escapes the project root")
        );
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
