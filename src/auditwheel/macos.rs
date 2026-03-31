//! macOS/Mach-O wheel audit and repair (delocate equivalent).
//!
//! This module implements [`WheelRepairer`] for macOS Mach-O binaries,
//! providing the Rust equivalent of [delocate](https://github.com/matthew-brett/delocate).
//!
//! Uses `arwen` for Mach-O install name / rpath manipulation and
//! `arwen-codesign` for pure-Rust ad-hoc code signing (no macOS tools needed).

use super::Policy;
use super::audit::relpath;
use super::repair::{AuditedArtifact, GraftedLib, WheelRepairer, leaf_filename};
use crate::compile::BuildArtifact;
use anyhow::{Context, Result};
use arwen::macho::MachoContainer;
use arwen_codesign::{AdhocSignOptions, adhoc_sign_file};
use lddtree::Library;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// macOS/Mach-O wheel repairer (delocate equivalent).
///
/// Bundles external `.dylib` files and rewrites Mach-O install names
/// and rpaths so that `@loader_path`-relative references resolve to
/// the bundled copies in the `.dylibs/` directory.
pub struct MacOSRepairer;

impl WheelRepairer for MacOSRepairer {
    fn audit(
        &self,
        artifact: &BuildArtifact,
        ld_paths: Vec<PathBuf>,
    ) -> Result<(Policy, Vec<Library>)> {
        let ext_libs = find_external_libs(&artifact.path, ld_paths)?;
        Ok((Policy::default(), ext_libs))
    }

    fn patch(
        &self,
        artifacts: &[AuditedArtifact],
        grafted: &[GraftedLib],
        libs_dir: &Path,
        artifact_dir: &Path,
    ) -> Result<()> {
        // Build a lookup from all known install names → new leaf name.
        let mut name_map: BTreeMap<&str, &str> = BTreeMap::new();
        for lib in grafted {
            name_map.insert(lib.original_name.as_str(), lib.new_name.as_str());
            for alias in &lib.aliases {
                name_map.insert(alias.as_str(), lib.new_name.as_str());
            }
        }

        // 1. Patch each grafted library: set install id, rewrite cross-references,
        //    remove absolute rpaths, then ad-hoc codesign.
        for lib in grafted {
            let new_install_id = format!("/DLC/{}/{}", libs_dir.display(), lib.new_name);

            // Collect rpaths to remove (all non-relative rpaths).
            let rpaths_to_remove: Vec<&str> = lib
                .rpath
                .iter()
                .filter(|r| !r.starts_with("@loader_path") && !r.starts_with("@executable_path"))
                .map(String::as_str)
                .collect();

            // Collect install name changes for cross-references between grafted libs.
            let install_name_changes: Vec<(&str, String)> = lib
                .needed
                .iter()
                .filter_map(|n| {
                    name_map
                        .get(n.as_str())
                        .map(|new| (n.as_str(), format!("@loader_path/{new}")))
                })
                .collect();

            patch_macho(
                &lib.dest_path,
                &install_name_changes,
                Some(&new_install_id),
                &rpaths_to_remove,
            )?;

            ad_hoc_sign(&lib.dest_path)?;
        }

        // 2. Patch each artifact: rewrite references to grafted libs using
        //    @loader_path-relative names.
        let rel = relpath(libs_dir, artifact_dir);
        for audited in artifacts {
            let install_name_changes: Vec<(&str, String)> = name_map
                .iter()
                .map(|(old, new)| {
                    let relative = Path::new("@loader_path").join(&rel).join(new);
                    (*old, relative.to_string_lossy().into_owned())
                })
                .collect();

            if !install_name_changes.is_empty() {
                patch_macho(&audited.artifact.path, &install_name_changes, None, &[])?;
                ad_hoc_sign(&audited.artifact.path)?;
            }
        }

        Ok(())
    }

    fn libs_dir(&self, dist_name: &str) -> PathBuf {
        PathBuf::from(format!("{dist_name}.dylibs"))
    }
}

/// Check if a library path is a macOS system library that should not be bundled.
///
/// System libraries live under `/usr/lib/` and `/System/`. Notably,
/// `/usr/local/lib/` (Homebrew) is NOT considered system — those libs
/// get bundled, matching Python delocate behaviour.
fn is_system_library(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.starts_with("/usr/lib/") || s.starts_with("/System/")
}

/// Check if a library name refers to libpython, which should never be bundled.
fn is_libpython(name: &str) -> bool {
    let leaf = leaf_filename(name);
    leaf.starts_with("libpython3")
}

/// Find external shared library dependencies for a macOS artifact.
fn find_external_libs(artifact: impl AsRef<Path>, ld_paths: Vec<PathBuf>) -> Result<Vec<Library>> {
    let analyzer = if ld_paths.is_empty() {
        lddtree::DependencyAnalyzer::default()
    } else {
        lddtree::DependencyAnalyzer::default().library_paths(ld_paths)
    };
    let deps = analyzer
        .analyze(artifact.as_ref())
        .context("Failed to analyze Mach-O dependencies")?;

    let mut ext_libs = Vec::new();
    for (_, lib) in deps.libraries {
        // Skip libraries that couldn't be resolved
        if lib.realpath.is_none() {
            continue;
        }
        // Skip system libraries
        if is_system_library(&lib.path) {
            continue;
        }
        // Skip libpython
        if is_libpython(&lib.name) {
            continue;
        }
        ext_libs.push(lib);
    }
    Ok(ext_libs)
}

/// Batch Mach-O patching: parse once, apply all changes, write once.
fn patch_macho(
    file: &Path,
    install_name_changes: &[(&str, String)],
    new_install_id: Option<&str>,
    rpaths_to_remove: &[&str],
) -> Result<()> {
    let data = fs_err::read(file)?;
    let mut container =
        MachoContainer::parse(&data).context("Failed to parse Mach-O for patching")?;

    if let Some(id) = new_install_id {
        // Ignore DylibIdMissing — the file may not be a dylib (e.g., a .so extension module).
        match container.change_install_id(id) {
            Ok(()) => {}
            Err(arwen::macho::MachoError::DylibIdMissing) => {}
            Err(e) => return Err(e).context("Failed to change install id"),
        }
    }

    for (old, new) in install_name_changes {
        // Ignore DylibNameMissing — the binary may not reference this name
        // (e.g., an alias that only appears in a different binary).
        match container.change_install_name(old, new) {
            Ok(()) => {}
            Err(arwen::macho::MachoError::DylibNameMissing(_)) => {}
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to change install name {old} -> {new}"));
            }
        }
    }

    for rpath in rpaths_to_remove {
        match container.remove_rpath(rpath) {
            Ok(()) => {}
            Err(arwen::macho::MachoError::RpathMissing(_)) => {}
            Err(e) => return Err(e).with_context(|| format!("Failed to remove rpath {rpath}")),
        }
    }

    fs_err::write(file, &container.data)?;
    Ok(())
}

/// Ad-hoc codesign a Mach-O binary using pure-Rust arwen-codesign.
fn ad_hoc_sign(file: &Path) -> Result<()> {
    let identifier = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    adhoc_sign_file(file, &AdhocSignOptions::new(identifier))
        .with_context(|| format!("Failed to ad-hoc codesign {}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_system_library() {
        assert!(is_system_library(Path::new("/usr/lib/libSystem.B.dylib")));
        assert!(is_system_library(Path::new(
            "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation"
        )));
        assert!(!is_system_library(Path::new("/usr/local/lib/libfoo.dylib")));
        assert!(!is_system_library(Path::new(
            "/opt/homebrew/lib/libbar.dylib"
        )));
    }

    #[test]
    fn test_is_libpython() {
        assert!(is_libpython("libpython3.12.dylib"));
        assert!(is_libpython("/usr/local/lib/libpython3.11.dylib"));
        assert!(is_libpython("@rpath/libpython3.10.dylib"));
        assert!(!is_libpython("libfoo.dylib"));
        assert!(!is_libpython("libpython2.7.dylib"));
    }
}
