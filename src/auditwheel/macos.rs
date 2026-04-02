//! macOS/Mach-O wheel audit and repair (delocate equivalent).
//!
//! This module implements [`WheelRepairer`] for macOS Mach-O binaries,
//! providing the Rust equivalent of [delocate](https://github.com/matthew-brett/delocate).
//!
//! Uses `arwen` for Mach-O install name / rpath manipulation and
//! pure-Rust signing helpers from `macos_sign` for both thin and fat binaries.

use super::Policy;
use super::audit::relpath;
use super::macos_sign::ad_hoc_sign;
use super::repair::{AuditedArtifact, GraftedLib, WheelRepairer, leaf_filename};
use crate::compile::BuildArtifact;
use anyhow::{Context, Result, bail};
use arwen::macho::MachoContainer;
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

/// Check if a library name refers to libpython or Python.framework, which should never be bundled.
///
/// This catches both:
/// - Traditional libpython: `/usr/local/lib/libpython3.12.dylib`, `@rpath/libpython3.10.dylib`
/// - Python framework: `/Library/Frameworks/Python.framework/Versions/3.14/Python`
fn is_libpython(name: &str) -> bool {
    // Check for Python.framework (macOS framework-style Python)
    if name.contains("Python.framework") {
        return true;
    }
    // Check for traditional libpython dylib
    let leaf = leaf_filename(name);
    leaf.starts_with("libpython3")
}

/// Decide whether a dependency should be bundled or ignored.
///
/// Unlike Linux, unresolved non-system Mach-O dependencies must fail the repair
/// because the resulting wheel would still be broken on another machine.
fn should_bundle_library(lib: &Library) -> Result<bool> {
    if is_system_library(&lib.path) || is_libpython(&lib.name) {
        return Ok(false);
    }

    if lib.realpath.is_none() {
        bail!(
            "Cannot repair wheel, because required library {} could not be located.",
            lib.path.display()
        );
    }

    Ok(true)
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
        if should_bundle_library(&lib)? {
            ext_libs.push(lib);
        }
    }
    Ok(ext_libs)
}

/// Batch Mach-O patching: apply changes, re-parsing between operations to handle
/// offset shifts from install name changes.
fn patch_macho(
    file: &Path,
    install_name_changes: &[(&str, String)],
    new_install_id: Option<&str>,
    rpaths_to_remove: &[&str],
) -> Result<()> {
    // Change install ID first (this can shift load command offsets)
    if let Some(id) = new_install_id {
        let data = fs_err::read(file)?;
        let mut container =
            MachoContainer::parse(&data).context("Failed to parse Mach-O for install_id change")?;
        match container.change_install_id(id) {
            Ok(()) => {
                fs_err::write(file, &container.data)?;
            }
            Err(arwen::macho::MachoError::DylibIdMissing) => {}
            Err(e) => return Err(e).context("Failed to change install id"),
        }
    }

    // Change install names (each can shift offsets, so re-parse between each)
    for (old, new) in install_name_changes {
        let data = fs_err::read(file)?;
        let mut container = MachoContainer::parse(&data)
            .context("Failed to parse Mach-O for install_name change")?;
        match container.change_install_name(old, new) {
            Ok(()) => {
                fs_err::write(file, &container.data)?;
            }
            Err(arwen::macho::MachoError::DylibNameMissing(_)) => {}
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to change install name {old} -> {new}"));
            }
        }
    }

    // Remove rpaths
    for rpath in rpaths_to_remove {
        let data = fs_err::read(file)?;
        let mut container =
            MachoContainer::parse(&data).context("Failed to parse Mach-O for rpath removal")?;
        match container.remove_rpath(rpath) {
            Ok(()) => {
                fs_err::write(file, &container.data)?;
            }
            Err(arwen::macho::MachoError::RpathMissing(_)) => {}
            Err(e) => return Err(e).with_context(|| format!("Failed to remove rpath {rpath}")),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(name: &str, path: &str, realpath: Option<&str>) -> Library {
        Library {
            name: name.to_string(),
            path: PathBuf::from(path),
            realpath: realpath.map(PathBuf::from),
            needed: Vec::new(),
            rpath: Vec::new(),
        }
    }

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
        // Traditional libpython dylibs
        assert!(is_libpython("libpython3.12.dylib"));
        assert!(is_libpython("/usr/local/lib/libpython3.11.dylib"));
        assert!(is_libpython("@rpath/libpython3.10.dylib"));
        // Python.framework (macOS framework-style Python)
        assert!(is_libpython(
            "/Library/Frameworks/Python.framework/Versions/3.14/Python"
        ));
        assert!(is_libpython(
            "/opt/homebrew/Frameworks/Python.framework/Versions/3.12/Python"
        ));
        // Non-Python libraries
        assert!(!is_libpython("libfoo.dylib"));
        assert!(!is_libpython("libpython2.7.dylib"));
    }

    #[test]
    fn test_missing_non_system_dependency_errors() {
        let err =
            should_bundle_library(&library("@rpath/libfoo.dylib", "@rpath/libfoo.dylib", None))
                .unwrap_err();

        assert_eq!(
            err.to_string(),
            "Cannot repair wheel, because required library @rpath/libfoo.dylib could not be located."
        );
    }

    #[test]
    fn test_missing_system_dependency_is_ignored() {
        assert!(
            !should_bundle_library(&library(
                "/usr/lib/libSystem.B.dylib",
                "/usr/lib/libSystem.B.dylib",
                None,
            ))
            .unwrap()
        );
    }
}
