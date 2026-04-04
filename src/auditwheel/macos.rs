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
            // Use a synthetic `/DLC/` (DeLoCated) install ID. This is a
            // non-existent absolute path used intentionally — matching the
            // convention from Python's `delocate` tool — so that any
            // un-patched reference will fail loudly at load time rather than
            // silently loading a different version of the library from the
            // system. All actual consumers are rewritten to use
            // `@loader_path`-relative references.
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
/// - Free-threaded Python framework: `/Library/Frameworks/PythonT.framework/Versions/3.14/PythonT`
fn is_libpython(name: &str) -> bool {
    // Check for Python.framework or PythonT.framework (macOS framework-style Python)
    // PythonT.framework is used by free-threaded Python builds (e.g., Python 3.13t, 3.14t)
    // Use path-component matching to avoid false positives on paths that merely
    // contain the substring (e.g., `/tmp/not-Python.framework-related/libfoo.dylib`).
    if Path::new(name).components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("Python.framework" | "PythonT.framework")
        )
    }) {
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
///
/// Uses a reachability analysis to avoid bundling libraries that are only
/// transitive dependencies of skipped libraries (e.g., `libintl` is a
/// dependency of `Python.framework` which we skip — so `libintl` should
/// not be bundled either unless something else we *are* bundling also
/// needs it).
fn find_external_libs(artifact: impl AsRef<Path>, ld_paths: Vec<PathBuf>) -> Result<Vec<Library>> {
    let analyzer = if ld_paths.is_empty() {
        lddtree::DependencyAnalyzer::default()
    } else {
        lddtree::DependencyAnalyzer::default().library_paths(ld_paths)
    };
    let deps = analyzer
        .analyze(artifact.as_ref())
        .context("Failed to analyze Mach-O dependencies")?;

    // Determine which libraries should be skipped.
    let skipped: std::collections::HashSet<&str> = deps
        .libraries
        .iter()
        .filter(|(_, lib)| is_system_library(&lib.path) || is_libpython(&lib.name))
        .map(|(name, _)| name.as_str())
        .collect();

    let reachable = reachable_libs(&deps.needed, &deps.libraries, &skipped);

    let mut ext_libs = Vec::new();
    for (name, lib) in &deps.libraries {
        if reachable.contains(name.as_str()) && should_bundle_library(lib)? {
            ext_libs.push(lib.clone());
        }
    }
    Ok(ext_libs)
}

/// BFS walk from `roots`, following `needed` edges in `libraries`, skipping
/// any node in `skipped`. Returns the set of reachable library names.
fn reachable_libs<'a>(
    roots: &'a [String],
    libraries: &'a std::collections::HashMap<String, Library>,
    skipped: &std::collections::HashSet<&str>,
) -> std::collections::HashSet<&'a str> {
    let mut reachable = std::collections::HashSet::new();
    let mut queue: std::collections::VecDeque<&str> = roots
        .iter()
        .filter(|n| !skipped.contains(n.as_str()))
        .map(String::as_str)
        .collect();
    while let Some(name) = queue.pop_front() {
        if !reachable.insert(name) {
            continue;
        }
        if let Some(lib) = libraries.get(name) {
            for needed in &lib.needed {
                if !skipped.contains(needed.as_str()) && !reachable.contains(needed.as_str()) {
                    queue.push_back(needed);
                }
            }
        }
    }
    reachable
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
        // PythonT.framework (free-threaded Python builds, e.g., 3.13t, 3.14t)
        assert!(is_libpython(
            "/Library/Frameworks/PythonT.framework/Versions/3.14/PythonT"
        ));
        assert!(is_libpython(
            "/opt/homebrew/Frameworks/PythonT.framework/Versions/3.13/PythonT"
        ));
        // Non-Python libraries
        assert!(!is_libpython("libfoo.dylib"));
        assert!(!is_libpython("libpython2.7.dylib"));
        // Paths that contain the substring but are not actual framework paths
        assert!(!is_libpython(
            "/tmp/not-Python.framework-related/libfoo.dylib"
        ));
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

    fn lib_with_needed(name: &str, path: &str, needed: &[&str]) -> Library {
        Library {
            name: name.to_string(),
            path: PathBuf::from(path),
            realpath: Some(PathBuf::from(path)),
            needed: needed.iter().map(|s| s.to_string()).collect(),
            rpath: Vec::new(),
        }
    }

    fn make_graph(libs: Vec<Library>) -> std::collections::HashMap<String, Library> {
        libs.into_iter().map(|l| (l.name.clone(), l)).collect()
    }

    #[test]
    fn skips_transitive_deps_of_skipped_libs() {
        // Graph: binary -> [libfoo, Python.framework]
        //        Python.framework -> [libintl]
        // libintl should NOT be reachable because it's only via a skipped lib.
        let libraries = make_graph(vec![
            lib_with_needed("libfoo.dylib", "/usr/local/lib/libfoo.dylib", &[]),
            lib_with_needed(
                "/Library/Frameworks/Python.framework/Versions/3.14/Python",
                "/Library/Frameworks/Python.framework/Versions/3.14/Python",
                &["libintl.8.dylib"],
            ),
            lib_with_needed("libintl.8.dylib", "/usr/local/lib/libintl.8.dylib", &[]),
        ]);
        let roots = vec![
            "libfoo.dylib".to_string(),
            "/Library/Frameworks/Python.framework/Versions/3.14/Python".to_string(),
        ];
        let skipped: std::collections::HashSet<&str> =
            ["/Library/Frameworks/Python.framework/Versions/3.14/Python"].into();

        let reachable = reachable_libs(&roots, &libraries, &skipped);
        assert!(reachable.contains("libfoo.dylib"));
        assert!(!reachable.contains("libintl.8.dylib"));
        assert!(!reachable.contains("/Library/Frameworks/Python.framework/Versions/3.14/Python"));
    }

    #[test]
    fn keeps_shared_dep_via_non_skipped_path() {
        // Graph: binary -> [libfoo, Python.framework]
        //        Python.framework -> [libintl]
        //        libfoo -> [libintl]
        // libintl IS reachable because libfoo (non-skipped) also needs it.
        let libraries = make_graph(vec![
            lib_with_needed(
                "libfoo.dylib",
                "/usr/local/lib/libfoo.dylib",
                &["libintl.8.dylib"],
            ),
            lib_with_needed(
                "/Library/Frameworks/Python.framework/Versions/3.14/Python",
                "/Library/Frameworks/Python.framework/Versions/3.14/Python",
                &["libintl.8.dylib"],
            ),
            lib_with_needed("libintl.8.dylib", "/usr/local/lib/libintl.8.dylib", &[]),
        ]);
        let roots = vec![
            "libfoo.dylib".to_string(),
            "/Library/Frameworks/Python.framework/Versions/3.14/Python".to_string(),
        ];
        let skipped: std::collections::HashSet<&str> =
            ["/Library/Frameworks/Python.framework/Versions/3.14/Python"].into();

        let reachable = reachable_libs(&roots, &libraries, &skipped);
        assert!(reachable.contains("libfoo.dylib"));
        assert!(reachable.contains("libintl.8.dylib"));
    }

    #[test]
    fn follows_transitive_chain() {
        // Graph: binary -> [libA]
        //        libA -> [libB]
        //        libB -> [libC]
        // All should be reachable.
        let libraries = make_graph(vec![
            lib_with_needed("libA.dylib", "/usr/local/lib/libA.dylib", &["libB.dylib"]),
            lib_with_needed("libB.dylib", "/usr/local/lib/libB.dylib", &["libC.dylib"]),
            lib_with_needed("libC.dylib", "/usr/local/lib/libC.dylib", &[]),
        ]);
        let roots = vec!["libA.dylib".to_string()];
        let skipped = std::collections::HashSet::new();

        let reachable = reachable_libs(&roots, &libraries, &skipped);
        assert!(reachable.contains("libA.dylib"));
        assert!(reachable.contains("libB.dylib"));
        assert!(reachable.contains("libC.dylib"));
    }

    #[test]
    fn skipped_mid_chain_blocks_descendants() {
        // Graph: binary -> [libA]
        //        libA -> [libSkip]
        //        libSkip -> [libC]
        // libC should NOT be reachable because libSkip blocks the path.
        let libraries = make_graph(vec![
            lib_with_needed(
                "libA.dylib",
                "/usr/local/lib/libA.dylib",
                &["/usr/lib/libSkip.dylib"],
            ),
            lib_with_needed(
                "/usr/lib/libSkip.dylib",
                "/usr/lib/libSkip.dylib",
                &["libC.dylib"],
            ),
            lib_with_needed("libC.dylib", "/usr/local/lib/libC.dylib", &[]),
        ]);
        let roots = vec!["libA.dylib".to_string()];
        let skipped: std::collections::HashSet<&str> = ["/usr/lib/libSkip.dylib"].into();

        let reachable = reachable_libs(&roots, &libraries, &skipped);
        assert!(reachable.contains("libA.dylib"));
        assert!(!reachable.contains("/usr/lib/libSkip.dylib"));
        assert!(!reachable.contains("libC.dylib"));
    }
}
