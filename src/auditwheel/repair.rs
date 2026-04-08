//! Shared wheel repair infrastructure.
//!
//! This module contains the [`WheelRepairer`] trait and the shared utilities
//! for preparing external libraries for grafting into wheels.
//!
//! Platform-specific implementations live in:
//! - [`super::linux::ElfRepairer`]
//! - [`super::macos::MacOSRepairer`]

use crate::compile::BuildArtifact;
use crate::util::hash_file;
use anyhow::{Context, Result};
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use fs_err as fs;

/// A build artifact bundled with the external shared libraries it depends on.
///
/// Keeps the artifact and its per-artifact dependency list together so they
/// cannot accidentally get out of sync when passed through the wheel-writing
/// pipeline.
pub struct AuditedArtifact {
    /// The build artifact.
    pub artifact: BuildArtifact,
    /// External shared libraries this artifact depends on that must be
    /// bundled into the wheel.
    pub external_libs: Vec<lddtree::Library>,
    /// **Universal2 only**: CPU architectures that require each library.
    ///
    /// Maps library realpath to the set of architectures (e.g., "arm64", "x86_64")
    /// that depend on it. Used during wheel repair to verify grafted dylibs
    /// contain all necessary architecture slices.
    ///
    /// Empty for single-arch builds and Linux builds.
    ///
    /// Note: Universal2 support may be removed when Apple drops x86_64 support
    pub arch_requirements: HashMap<PathBuf, HashSet<String>>,
}

impl Borrow<BuildArtifact> for AuditedArtifact {
    fn borrow(&self) -> &BuildArtifact {
        &self.artifact
    }
}

/// A library prepared for grafting into a wheel.
///
/// Created by [`prepare_grafted_libs`] with a hash-suffixed filename and a
/// writable temporary copy ready for platform-specific patching.
pub struct GraftedLib {
    /// Original library name as it appears in dependency records.
    /// For ELF this is a leaf name like `libfoo.so.1`.
    /// For Mach-O this can be a full install name like `/usr/local/lib/libfoo.dylib`
    /// or `@rpath/libfoo.dylib`.
    pub original_name: String,
    /// Additional install names that resolve to the same file on disk.
    /// These need the same rewriting as `original_name` → `new_name`.
    pub aliases: Vec<String>,
    /// New filename with hash suffix (e.g., `libfoo-ab12cd34.so.1`)
    pub new_name: String,
    /// Path to the writable temporary copy (ready for patching).
    pub dest_path: PathBuf,
    /// Libraries this one depends on (from lddtree's `needed` field).
    pub needed: Vec<String>,
    /// Runtime library search paths from the original library.
    pub rpath: Vec<String>,
    /// **Universal2 only**: CPU architectures that require this library.
    ///
    /// For universal2 macOS wheels, each architecture (arm64, x86_64) may have
    /// different dependencies. This field tracks which architectures actually
    /// need this library, so we can verify the grafted dylib contains (at least)
    /// those architectures.
    ///
    /// Empty for single-arch builds and Linux builds.
    ///
    /// Note: Universal2 support may be removed when Apple drops x86_64 support
    /// (expected ~2025-2026).
    pub required_archs: HashSet<String>,
}

/// Result of auditing a build artifact for external dependencies.
///
/// Contains the platform policy, discovered external libraries, and
/// (for universal2 macOS builds) architecture requirements.
pub struct AuditResult {
    /// The determined platform policy (e.g., manylinux tag).
    pub policy: super::Policy,
    /// External shared libraries that need to be bundled.
    pub external_libs: Vec<lddtree::Library>,
    /// **Universal2 only**: CPU architectures that require each library.
    ///
    /// Maps library realpath to the set of architectures (e.g., "arm64", "x86_64")
    /// that depend on it. Empty for single-arch builds and Linux builds.
    ///
    /// Note: Universal2 support may be removed when Apple drops x86_64 support
    pub arch_requirements: HashMap<PathBuf, HashSet<String>>,
}

impl AuditResult {
    /// Create a new AuditResult with no arch requirements (for single-arch/Linux).
    pub fn new(policy: super::Policy, external_libs: Vec<lddtree::Library>) -> Self {
        Self {
            policy,
            external_libs,
            arch_requirements: HashMap::new(),
        }
    }
}

/// Platform-specific wheel repair operations.
///
/// Each platform (Linux/ELF, macOS/Mach-O) implements this trait to provide
/// its own dependency discovery and binary patching logic.
pub trait WheelRepairer {
    /// Audit an artifact for platform compliance and find external libraries
    /// that need to be bundled.
    ///
    /// Returns an [`AuditResult`] containing the platform policy, external
    /// library dependencies, and (for universal2) architecture requirements.
    fn audit(&self, artifact: &BuildArtifact, ld_paths: Vec<PathBuf>) -> Result<AuditResult>;

    /// Patch binary references after libraries have been grafted.
    ///
    /// This is called after [`prepare_grafted_libs`] has copied and
    /// hash-renamed all external libraries. Implementations should:
    ///
    /// 1. Rewrite references in each artifact to point to the new names
    /// 2. Set appropriate metadata on grafted libraries (soname, install ID, etc.)
    /// 3. Update cross-references between grafted libraries
    /// 4. Perform any final steps (e.g., code signing on macOS)
    fn patch(
        &self,
        audited: &[AuditedArtifact],
        grafted: &[GraftedLib],
        libs_dir: &Path,
        artifact_dir: &Path,
    ) -> Result<()>;

    /// Patch artifacts for editable installs (e.g., set RPATH to Cargo target dir).
    ///
    /// The default implementation is a no-op. Platform-specific repairers can
    /// override this to add runtime library search paths for editable mode.
    fn patch_editable(&self, _audited: &[AuditedArtifact]) -> Result<()> {
        Ok(())
    }

    /// Return a Python code snippet to prepend to `__init__.py` for runtime
    /// shared library discovery.
    ///
    /// `libs_dir_name` is the leaf directory name for bundled libraries (e.g.
    /// `"mypackage.libs"`). `depth` is the number of parent directories from
    /// the package's `__init__.py` to the site-packages root where `.libs/`
    /// lives.
    ///
    /// Returns `None` on platforms that don't need runtime patching:
    /// - Linux/ELF uses RPATH (`$ORIGIN`)
    /// - macOS/Mach-O uses `@loader_path`
    /// - Windows/PE needs `os.add_dll_directory()` injected into `__init__.py`
    fn init_py_patch(&self, _libs_dir_name: &str, _depth: usize) -> Option<String> {
        None
    }

    /// Return the wheel-internal directory name for grafted libraries.
    ///
    /// macOS uses `.dylibs` (matching delocate convention),
    /// Linux and Windows use `.libs` (matching auditwheel/delvewheel convention).
    fn libs_dir(&self, dist_name: &str) -> PathBuf {
        PathBuf::from(format!("{dist_name}.libs"))
    }
}

/// Prepare external libraries for grafting into a wheel.
///
/// For each library:
/// 1. Resolves the real path on disk (fails if not found)
/// 2. Generates a hash-suffixed filename to avoid DLL hell
/// 3. Copies to `temp_dir` and makes the copy writable
///
/// Returns the prepared libraries and the set of original paths that were copied.
///
/// Deduplication is by `realpath` (the actual file on disk). When the same
/// file is referenced via multiple install names (common on macOS), only one
/// copy is made, but all original names are recorded as aliases.
///
/// The optional `arch_requirements` parameter is used for universal2 macOS builds
/// to track which CPU architectures require each library (by realpath). This
/// enables verification that grafted dylibs contain all necessary architecture
/// slices. For single-arch builds or Linux, pass `None`.
pub fn prepare_grafted_libs(
    audited: &[AuditedArtifact],
    temp_dir: &Path,
    arch_requirements: Option<&HashMap<PathBuf, HashSet<String>>>,
) -> Result<(Vec<GraftedLib>, HashSet<PathBuf>)> {
    let mut grafted = Vec::new();
    let mut libs_copied = HashSet::new();
    let mut realpath_to_idx: HashMap<PathBuf, usize> = HashMap::new();

    for lib in audited.iter().flat_map(|a| &a.external_libs) {
        let source_path = lib.realpath.clone().with_context(|| {
            format!(
                "Cannot repair wheel, because required library {} could not be located.",
                lib.path.display()
            )
        })?;

        // Check if we've already copied this exact file (by realpath).
        if let Some(&idx) = realpath_to_idx.get(&source_path) {
            let existing: &mut GraftedLib = &mut grafted[idx];
            if lib.name != existing.original_name && !existing.aliases.contains(&lib.name) {
                existing.aliases.push(lib.name.clone());
            }
            continue;
        }

        let new_name = hashed_lib_name(&lib.name, &source_path)?;
        let dest_path = temp_dir.join(&new_name);

        fs::copy(&source_path, &dest_path)?;
        // Make the copy writable so platform-specific tools can modify it
        let mut perms = fs::metadata(&dest_path)?.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&dest_path, perms)?;

        let idx = grafted.len();
        realpath_to_idx.insert(source_path.clone(), idx);
        libs_copied.insert(source_path.clone());

        // Get required architectures for this library (universal2 only).
        let required_archs = arch_requirements
            .and_then(|reqs| reqs.get(&source_path))
            .cloned()
            .unwrap_or_default();

        grafted.push(GraftedLib {
            original_name: lib.name.clone(),
            aliases: Vec::new(),
            new_name,
            dest_path,
            needed: lib.needed.clone(),
            rpath: lib.rpath.clone(),
            required_archs,
        });
    }

    Ok((grafted, libs_copied))
}

/// Extract the leaf filename from a library name.
///
/// Library names can be full paths on macOS (e.g., `/usr/local/lib/libfoo.dylib`
/// or `@rpath/libfoo.dylib`). This returns just the filename component.
pub(crate) fn leaf_filename(lib_name: &str) -> &str {
    Path::new(lib_name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(lib_name)
}

/// Generate a hash-suffixed filename for a library to avoid collisions.
///
/// Takes the leaf filename from `lib_name` (which may be a full path on macOS),
/// computes a short hash of the file content, and inserts it before the first
/// extension dot.
///
/// Examples:
/// - `libfoo.so.1` + hash `ab12cd34` → `libfoo-ab12cd34.so.1`
/// - `/usr/local/lib/libbar.dylib` + hash `ef56gh78` → `libbar-ef56gh78.dylib`
pub(crate) fn hashed_lib_name(lib_name: &str, lib_path: &Path) -> Result<String> {
    let short_hash = &hash_file(lib_path)
        .with_context(|| format!("Failed to hash library {}", lib_path.display()))?[..8];

    let leaf = leaf_filename(lib_name);

    Ok(if let Some(pos) = leaf.find('.') {
        let (stem, ext) = leaf.split_at(pos);
        if stem.ends_with(&format!("-{short_hash}")) {
            leaf.to_string()
        } else {
            format!("{stem}-{short_hash}{ext}")
        }
    } else {
        format!("{leaf}-{short_hash}")
    })
}

/// Log which libraries were grafted into the wheel.
pub fn log_grafted_libs(libs_copied: &HashSet<PathBuf>, libs_dir: &Path) {
    let mut grafted_paths: Vec<&PathBuf> = libs_copied.iter().collect();
    grafted_paths.sort();

    eprintln!(
        "🖨  Copied external shared libraries to package {} directory:",
        libs_dir.display()
    );
    for lib_path in &grafted_paths {
        eprintln!("    {}", lib_path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_leaf_filename() {
        assert_eq!(leaf_filename("libfoo.so.1"), "libfoo.so.1");
        assert_eq!(leaf_filename("/usr/local/lib/libfoo.dylib"), "libfoo.dylib");
        assert_eq!(leaf_filename("@rpath/libfoo.dylib"), "libfoo.dylib");
    }

    #[test]
    fn test_hashed_lib_name() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let lib_path = tmp_dir.path().join("libfoo.so.1");
        {
            let mut f = fs_err::File::create(&lib_path).unwrap();
            f.write_all(b"fake library content").unwrap();
        }
        let name = hashed_lib_name("libfoo.so.1", &lib_path).unwrap();
        // Should have format: libfoo-XXXXXXXX.so.1
        assert!(name.starts_with("libfoo-"));
        assert!(name.ends_with(".so.1"));
        assert_eq!(name.len(), "libfoo-".len() + 8 + ".so.1".len());

        // Idempotent: calling with already-hashed name should not double-hash
        let name2 = hashed_lib_name(&name, &lib_path).unwrap();
        assert_eq!(name, name2);
    }

    #[test]
    fn test_hashed_lib_name_macos_path() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let lib_path = tmp_dir.path().join("libbar.dylib");
        {
            let mut f = fs_err::File::create(&lib_path).unwrap();
            f.write_all(b"fake dylib content").unwrap();
        }
        let name = hashed_lib_name("/usr/local/lib/libbar.dylib", &lib_path).unwrap();
        assert!(name.starts_with("libbar-"));
        assert!(name.ends_with(".dylib"));
    }
}
