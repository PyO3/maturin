//! macOS/Mach-O wheel audit and repair (delocate equivalent).
//!
//! This module implements [`WheelRepairer`] for macOS Mach-O binaries,
//! providing the Rust equivalent of [delocate](https://github.com/matthew-brett/delocate).
//!
//! Uses `arwen` for Mach-O install name / rpath manipulation and
//! pure-Rust signing helpers from `macos_sign` for both thin and fat binaries.

use super::Policy;
use super::audit::{get_sysroot_path, relpath};
use super::macos_sign::ad_hoc_sign;
use super::repair::{AuditResult, AuditedArtifact, GraftedLib, WheelRepairer, leaf_filename};
use crate::compile::BuildArtifact;
use crate::target::Target;
use anyhow::{Context, Result, bail};
use arwen::macho::MachoContainer;
use fat_macho::{Error as FatMachoError, FatReader};
use lddtree::Library;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// macOS/Mach-O wheel repairer (delocate equivalent).
///
/// Bundles external `.dylib` files and rewrites Mach-O install names
/// and rpaths so that `@loader_path`-relative references resolve to
/// the bundled copies in the `.dylibs/` directory.
pub struct MacOSRepairer {
    /// The build target, used to determine the sysroot for cross-compilation.
    pub target: Target,
}

impl WheelRepairer for MacOSRepairer {
    fn audit(&self, artifact: &BuildArtifact, _ld_paths: Vec<PathBuf>) -> Result<AuditResult> {
        let sysroot = get_sysroot_path(&self.target).unwrap_or_else(|_| PathBuf::from("/"));

        // For universal2 builds, analyze each thin binary separately with its
        // own linked_paths. This ensures we discover dependencies that may
        // only exist in one architecture (e.g., arch-specific native libs).
        // lddtree can only analyze one arch at a time from a fat binary.
        if !artifact.thin_artifacts.is_empty() {
            let mut all_libs: Vec<Library> = Vec::new();
            let mut seen_realpaths: HashMap<PathBuf, usize> = HashMap::new();
            let mut arch_requirements: HashMap<PathBuf, HashSet<String>> = HashMap::new();

            for thin in &artifact.thin_artifacts {
                let ld_paths: Vec<PathBuf> = thin.linked_paths.iter().map(PathBuf::from).collect();
                let libs = find_external_libs(&thin.path, ld_paths, &sysroot)?;
                for lib in libs {
                    if let Some(ref realpath) = lib.realpath {
                        // Track which architectures require this library.
                        arch_requirements
                            .entry(realpath.clone())
                            .or_default()
                            .insert(thin.arch.clone());

                        // Deduplicate by realpath — the same dylib may be needed
                        // by both arches but should only be bundled once.
                        if !seen_realpaths.contains_key(realpath) {
                            seen_realpaths.insert(realpath.clone(), all_libs.len());
                            all_libs.push(lib);
                        }
                    } else {
                        // Library not found on disk; include it so the error
                        // propagates later in prepare_grafted_libs.
                        all_libs.push(lib);
                    }
                }
            }

            Ok(AuditResult {
                policy: Policy::default(),
                external_libs: all_libs,
                arch_requirements,
            })
        } else {
            // Single-arch build: analyze the artifact directly.
            let ld_paths: Vec<PathBuf> = artifact.linked_paths.iter().map(PathBuf::from).collect();
            let ext_libs = find_external_libs(&artifact.path, ld_paths, &sysroot)?;
            Ok(AuditResult::new(Policy::default(), ext_libs))
        }
    }

    fn patch(
        &self,
        artifacts: &[AuditedArtifact],
        grafted: &[GraftedLib],
        libs_dir: &Path,
        artifact_dir: &Path,
    ) -> Result<()> {
        // Verify universal2 architecture requirements before patching.
        // Each grafted library must contain all CPU architectures that depend on it.
        for lib in grafted {
            if !lib.required_archs.is_empty() {
                verify_universal_archs(&lib.dest_path, &lib.required_archs, &lib.original_name)?;
            }
        }

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

            // Collect rpaths to remove (absolute rpaths only).
            let rpaths_to_remove: Vec<&str> = lib
                .rpath
                .iter()
                .filter(|r| r.starts_with('/'))
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

/// **Universal2 only**: Verify that a dylib contains all required CPU architectures.
///
/// For universal2 macOS wheels, each bundled dylib must contain the architecture
/// slices that depend on it. For example, if both arm64 and x86_64 binaries link
/// against libfoo.dylib, then libfoo.dylib must be a fat binary containing both
/// arm64 and x86_64 slices.
///
/// This mirrors Python delocate's `check_archs` behavior — it assumes that
/// external dylibs on the build system are already fat/universal. If a dylib
/// is thin (single-architecture), the wheel repair will fail with an error
/// explaining which architecture is missing.
///
/// Note: Universal2 support may be removed when Apple drops x86_64 support
fn verify_universal_archs(
    path: &Path,
    required_archs: &HashSet<String>,
    lib_name: &str,
) -> Result<()> {
    let data = fs_err::read(path).with_context(|| {
        format!(
            "Failed to read library for arch verification: {}",
            path.display()
        )
    })?;

    match FatReader::new(&data) {
        Ok(reader) => {
            // Fat binary: check each required arch is present
            for arch in required_archs {
                if reader.extract(arch).is_none() {
                    bail!(
                        "Library '{}' is missing architecture '{}'. \
                         Universal2 wheels require fat/universal dylibs containing all \
                         required architectures. The wheel's arm64 and/or x86_64 binaries \
                         depend on this library, but it doesn't contain a '{}' slice.",
                        lib_name,
                        arch,
                        arch
                    );
                }
            }
        }
        Err(FatMachoError::NotFatBinary) => {
            // Thin binary: fails if more than one arch is required
            if required_archs.len() > 1 {
                let archs: Vec<&str> = required_archs.iter().map(|s| s.as_str()).collect();
                bail!(
                    "Library '{}' is a thin (single-architecture) binary, but the universal2 \
                     wheel requires architectures: {}. Universal2 wheels need fat/universal \
                     dylibs. Install a universal version of this library or build separate \
                     wheels for each architecture instead of universal2.",
                    lib_name,
                    archs.join(", ")
                );
            }
            // Single arch required and it's a thin binary — we assume it matches.
            // (If it didn't match, the build would have failed earlier.)
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!("Failed to parse Mach-O for arch verification: {lib_name}")
            });
        }
    }

    Ok(())
}

/// Check if a library path is a macOS system library that should not be bundled.
///
/// System libraries live under `/usr/lib/` and `/System/`. Notably,
/// `/usr/local/lib/` (Homebrew) is NOT considered system — those libs
/// get bundled, matching Python delocate behaviour.
///
/// When cross-compiling, resolved library paths may be prefixed with the SDK
/// sysroot (e.g., `/opt/osxcross/SDK/MacOSX.sdk/usr/lib/…`). The `sysroot`
/// parameter is stripped before checking.
fn is_system_library(path: &Path, sysroot: &Path) -> bool {
    let effective = path
        .strip_prefix(sysroot)
        .map(|p| Path::new("/").join(p))
        .unwrap_or_else(|_| path.to_path_buf());
    let s = effective.to_string_lossy();
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
fn should_bundle_library(lib: &Library, sysroot: &Path) -> Result<bool> {
    if is_system_library(&lib.path, sysroot) || is_libpython(&lib.name) {
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
fn find_external_libs(
    artifact: impl AsRef<Path>,
    ld_paths: Vec<PathBuf>,
    sysroot: &Path,
) -> Result<Vec<Library>> {
    let analyzer = if ld_paths.is_empty() {
        lddtree::DependencyAnalyzer::default()
    } else {
        lddtree::DependencyAnalyzer::default().library_paths(ld_paths)
    };
    let deps = analyzer
        .analyze(artifact.as_ref())
        .context("Failed to analyze Mach-O dependencies")?;

    // Determine which libraries should be skipped.
    let skipped: HashSet<&str> = deps
        .libraries
        .iter()
        .filter(|(_, lib)| is_system_library(&lib.path, sysroot) || is_libpython(&lib.name))
        .map(|(name, _)| name.as_str())
        .collect();

    let reachable = reachable_libs(&deps.needed, &deps.libraries, &skipped);

    let mut ext_libs = Vec::new();
    for (name, lib) in &deps.libraries {
        if reachable.contains(name.as_str()) && should_bundle_library(lib, sysroot)? {
            ext_libs.push(lib.clone());
        }
    }
    Ok(ext_libs)
}

/// BFS walk from `roots`, following `needed` edges in `libraries`, skipping
/// any node in `skipped`. Returns the set of reachable library names.
fn reachable_libs<'a>(
    roots: &'a [String],
    libraries: &'a HashMap<String, Library>,
    skipped: &HashSet<&str>,
) -> HashSet<&'a str> {
    let mut reachable = HashSet::new();
    let mut queue: VecDeque<&str> = roots
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

/// Batch Mach-O patching: apply changes in-memory, re-parsing between operations
/// to handle offset shifts from install name changes, then write once at the end.
fn patch_macho(
    file: &Path,
    install_name_changes: &[(&str, String)],
    new_install_id: Option<&str>,
    rpaths_to_remove: &[&str],
) -> Result<()> {
    let mut data = fs_err::read(file)?;
    let mut modified = false;

    // Change install ID first (this can shift load command offsets)
    if let Some(id) = new_install_id {
        let mut container =
            MachoContainer::parse(&data).context("Failed to parse Mach-O for install_id change")?;
        match container.change_install_id(id) {
            Ok(()) => {
                data = container.data;
                modified = true;
            }
            Err(arwen::macho::MachoError::DylibIdMissing) => {}
            Err(e) => return Err(e).context("Failed to change install id"),
        }
    }

    // Change install names (each can shift offsets, so re-parse between each)
    for (old, new) in install_name_changes {
        let mut container = MachoContainer::parse(&data)
            .context("Failed to parse Mach-O for install_name change")?;
        match container.change_install_name(old, new) {
            Ok(()) => {
                data = container.data;
                modified = true;
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
        let mut container =
            MachoContainer::parse(&data).context("Failed to parse Mach-O for rpath removal")?;
        match container.remove_rpath(rpath) {
            Ok(()) => {
                data = container.data;
                modified = true;
            }
            Err(arwen::macho::MachoError::RpathMissing(_)) => {}
            Err(e) => return Err(e).with_context(|| format!("Failed to remove rpath {rpath}")),
        }
    }

    // Write once at the end if any modifications were made
    if modified {
        fs_err::write(file, &data)?;
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
        let root = Path::new("/");
        assert!(is_system_library(
            Path::new("/usr/lib/libSystem.B.dylib"),
            root
        ));
        assert!(is_system_library(
            Path::new("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation"),
            root,
        ));
        assert!(!is_system_library(
            Path::new("/usr/local/lib/libfoo.dylib"),
            root
        ));
        assert!(!is_system_library(
            Path::new("/opt/homebrew/lib/libbar.dylib"),
            root,
        ));
    }

    #[test]
    fn test_is_system_library_with_sysroot() {
        let sysroot = Path::new("/opt/osxcross/SDK/MacOSX.sdk");
        assert!(is_system_library(
            Path::new("/opt/osxcross/SDK/MacOSX.sdk/usr/lib/libSystem.B.dylib"),
            sysroot,
        ));
        assert!(is_system_library(
            Path::new(
                "/opt/osxcross/SDK/MacOSX.sdk/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation"
            ),
            sysroot,
        ));
        assert!(!is_system_library(
            Path::new("/opt/osxcross/SDK/MacOSX.sdk/usr/local/lib/libfoo.dylib"),
            sysroot,
        ));
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
        let root = Path::new("/");
        let err = should_bundle_library(
            &library("@rpath/libfoo.dylib", "@rpath/libfoo.dylib", None),
            root,
        )
        .unwrap_err();

        assert_eq!(
            err.to_string(),
            "Cannot repair wheel, because required library @rpath/libfoo.dylib could not be located."
        );
    }

    #[test]
    fn test_missing_system_dependency_is_ignored() {
        let root = Path::new("/");
        assert!(
            !should_bundle_library(
                &library(
                    "/usr/lib/libSystem.B.dylib",
                    "/usr/lib/libSystem.B.dylib",
                    None,
                ),
                root,
            )
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

    fn make_graph(libs: Vec<Library>) -> HashMap<String, Library> {
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
        let skipped: HashSet<&str> =
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
        let skipped: HashSet<&str> =
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
        let skipped = HashSet::new();

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
        let skipped: HashSet<&str> = ["/usr/lib/libSkip.dylib"].into();

        let reachable = reachable_libs(&roots, &libraries, &skipped);
        assert!(reachable.contains("libA.dylib"));
        assert!(!reachable.contains("/usr/lib/libSkip.dylib"));
        assert!(!reachable.contains("libC.dylib"));
    }

    // Tests for verify_universal_archs
    mod verify_universal_archs_tests {
        use super::*;
        use fat_macho::FatWriter;
        use std::io::Write;

        /// Minimal Mach-O header for arm64 (enough to be recognized as valid thin Mach-O)
        fn minimal_arm64_macho() -> Vec<u8> {
            // MH_MAGIC_64 + CPU_TYPE_ARM64 + minimal header
            let mut data = Vec::new();
            // MH_MAGIC_64 = 0xfeedfacf (little-endian)
            data.extend_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
            // CPU_TYPE_ARM64 = 0x0100000c (little-endian)
            data.extend_from_slice(&[0x0c, 0x00, 0x00, 0x01]);
            // CPU_SUBTYPE_ARM64_ALL = 0
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            // MH_EXECUTE = 2
            data.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]);
            // ncmds = 0
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            // sizeofcmds = 0
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            // flags = 0
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            // reserved = 0
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            data
        }

        /// Minimal Mach-O header for x86_64
        fn minimal_x86_64_macho() -> Vec<u8> {
            let mut data = Vec::new();
            // MH_MAGIC_64 = 0xfeedfacf (little-endian)
            data.extend_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
            // CPU_TYPE_X86_64 = 0x01000007 (little-endian)
            data.extend_from_slice(&[0x07, 0x00, 0x00, 0x01]);
            // CPU_SUBTYPE_X86_64_ALL = 3
            data.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]);
            // MH_EXECUTE = 2
            data.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]);
            // ncmds = 0
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            // sizeofcmds = 0
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            // flags = 0
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            // reserved = 0
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            data
        }

        /// Create a fat binary from arm64 and x86_64 slices
        fn create_fat_binary(arm64: Vec<u8>, x86_64: Vec<u8>) -> Vec<u8> {
            let mut writer = FatWriter::new();
            writer.add(arm64).unwrap();
            writer.add(x86_64).unwrap();
            let mut output = Vec::new();
            writer.write_to(&mut output).unwrap();
            output
        }

        #[test]
        fn fat_binary_with_both_archs_passes() {
            let tmp_dir = tempfile::tempdir().unwrap();
            let lib_path = tmp_dir.path().join("libfoo.dylib");

            let fat_binary = create_fat_binary(minimal_arm64_macho(), minimal_x86_64_macho());
            fs_err::write(&lib_path, fat_binary).unwrap();

            let required: HashSet<String> =
                ["arm64", "x86_64"].iter().map(|s| s.to_string()).collect();
            let result = verify_universal_archs(&lib_path, &required, "libfoo.dylib");
            assert!(result.is_ok());
        }

        #[test]
        fn fat_binary_missing_arch_fails() {
            let tmp_dir = tempfile::tempdir().unwrap();
            let lib_path = tmp_dir.path().join("libfoo.dylib");

            // Create fat binary with only arm64 (not a real fat, just arm64 thin)
            // For this test, we use a fat binary with only arm64
            let mut writer = FatWriter::new();
            writer.add(minimal_arm64_macho()).unwrap();
            let mut output = Vec::new();
            writer.write_to(&mut output).unwrap();
            fs_err::write(&lib_path, output).unwrap();

            let required: HashSet<String> =
                ["arm64", "x86_64"].iter().map(|s| s.to_string()).collect();
            let result = verify_universal_archs(&lib_path, &required, "libfoo.dylib");
            assert!(result.is_err());
            let err_msg = result.unwrap_err().to_string();
            assert!(err_msg.contains("missing architecture"));
            assert!(err_msg.contains("x86_64"));
        }

        #[test]
        fn thin_binary_with_multiple_required_archs_fails() {
            let tmp_dir = tempfile::tempdir().unwrap();
            let lib_path = tmp_dir.path().join("libfoo.dylib");

            // Write a thin arm64 binary
            fs_err::write(&lib_path, minimal_arm64_macho()).unwrap();

            let required: HashSet<String> =
                ["arm64", "x86_64"].iter().map(|s| s.to_string()).collect();
            let result = verify_universal_archs(&lib_path, &required, "libfoo.dylib");
            assert!(result.is_err());
            let err_msg = result.unwrap_err().to_string();
            assert!(err_msg.contains("thin (single-architecture)"));
        }

        #[test]
        fn thin_binary_with_single_required_arch_passes() {
            let tmp_dir = tempfile::tempdir().unwrap();
            let lib_path = tmp_dir.path().join("libfoo.dylib");

            // Write a thin arm64 binary
            fs_err::write(&lib_path, minimal_arm64_macho()).unwrap();

            // Only arm64 required - should pass (we assume it matches)
            let required: HashSet<String> = ["arm64"].iter().map(|s| s.to_string()).collect();
            let result = verify_universal_archs(&lib_path, &required, "libfoo.dylib");
            assert!(result.is_ok());
        }

        #[test]
        fn invalid_macho_fails_with_parse_error() {
            let tmp_dir = tempfile::tempdir().unwrap();
            let lib_path = tmp_dir.path().join("libfoo.dylib");

            // Write invalid data (not a Mach-O)
            let mut f = fs_err::File::create(&lib_path).unwrap();
            f.write_all(b"not a macho file").unwrap();

            let required: HashSet<String> = ["arm64"].iter().map(|s| s.to_string()).collect();
            let result = verify_universal_archs(&lib_path, &required, "libfoo.dylib");
            assert!(result.is_err());
            let err_msg = result.unwrap_err().to_string();
            assert!(err_msg.contains("Failed to parse Mach-O"));
        }
    }
}
