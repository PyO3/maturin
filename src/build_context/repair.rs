#[cfg(feature = "auditwheel")]
use crate::auditwheel::MacOSRepairer;
#[cfg(feature = "auditwheel")]
use crate::auditwheel::WindowsRepairer;
#[cfg(feature = "sbom")]
use crate::auditwheel::get_sysroot_path;
use crate::auditwheel::{
    AuditResult, AuditWheelMode, AuditedArtifact, ElfRepairer, PlatformTag, Policy, WheelRepairer,
    log_grafted_libs, prepare_grafted_libs,
};
#[cfg(feature = "sbom")]
use crate::module_writer::ModuleWriter;
use crate::module_writer::WheelWriter;
use crate::{BridgeModel, BuildArtifact, PythonInterpreter, VirtualWriter};
use anyhow::{Context, Result, bail};
use fs_err as fs;
use normpath::PathExt;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use super::BuildContext;

impl BuildContext {
    /// Create the appropriate platform-specific wheel repairer.
    fn make_repairer(
        &self,
        platform_tag: &[PlatformTag],
        python_interpreter: Option<&PythonInterpreter>,
    ) -> Option<Box<dyn WheelRepairer>> {
        if self.project.target.is_linux() {
            let mut musllinux: Vec<_> = platform_tag
                .iter()
                .filter(|tag| tag.is_musllinux())
                .copied()
                .collect();
            musllinux.sort();
            let mut others: Vec<_> = platform_tag
                .iter()
                .filter(|tag| !tag.is_musllinux())
                .copied()
                .collect();
            others.sort();

            let allow_linking_libpython = self.project.bridge().is_bin();

            let effective_tag = if self.project.bridge().is_bin() && !musllinux.is_empty() {
                Some(musllinux[0])
            } else {
                others.first().or_else(|| musllinux.first()).copied()
            };

            Some(Box::new(ElfRepairer {
                platform_tag: effective_tag,
                target: self.project.target.clone(),
                manifest_path: self.project.manifest_path.clone(),
                allow_linking_libpython,
            }))
        } else if self.project.target.is_macos() {
            #[cfg(feature = "auditwheel")]
            {
                Some(Box::new(MacOSRepairer {
                    target: self.project.target.clone(),
                }))
            }
            #[cfg(not(feature = "auditwheel"))]
            {
                None
            }
        } else if self.project.target.is_windows() {
            #[cfg(feature = "auditwheel")]
            {
                let is_pypy = python_interpreter
                    .map(|p| p.interpreter_kind.is_pypy())
                    .unwrap_or(false);
                Some(Box::new(WindowsRepairer { is_pypy }))
            }
            #[cfg(not(feature = "auditwheel"))]
            {
                None
            }
        } else {
            None
        }
    }

    pub(crate) fn auditwheel(
        &self,
        artifact: &BuildArtifact,
        platform_tag: &[PlatformTag],
        python_interpreter: Option<&PythonInterpreter>,
    ) -> Result<AuditResult> {
        if matches!(self.python.auditwheel, AuditWheelMode::Skip) {
            return Ok(AuditResult::new(Policy::default(), Vec::new()));
        }

        if let Some(python_interpreter) = python_interpreter
            && platform_tag.is_empty()
            && self.project.target.is_linux()
            && !python_interpreter.support_portable_wheels()
        {
            eprintln!(
                "🐍 Skipping auditwheel because {python_interpreter} does not support manylinux/musllinux wheels"
            );
            return Ok(AuditResult::new(Policy::default(), Vec::new()));
        }

        let repairer = match self.make_repairer(platform_tag, python_interpreter) {
            Some(r) => r,
            None => return Ok(AuditResult::new(Policy::default(), Vec::new())),
        };

        let ld_paths: Vec<PathBuf> = artifact.linked_paths.iter().map(PathBuf::from).collect();
        repairer.audit(artifact, ld_paths)
    }

    /// Compute the wheel-internal directory where the artifact resides.
    fn get_artifact_dir(&self) -> PathBuf {
        match self.project.bridge() {
            // cffi bindings that contains '.' in the module name will be split into directories
            BridgeModel::Cffi => self.project.module_name.split(".").collect::<PathBuf>(),
            // For namespace packages the modules reside at ${module_name}.so
            // where periods are replaced with slashes so for example my.namespace.module would reside
            // at my/namespace/module.so
            _ if self.project.module_name.contains(".") => {
                let mut path = self.project.module_name.split(".").collect::<PathBuf>();
                path.pop();
                path
            }
            // For other bindings artifact .so file usually resides at ${module_name}/${module_name}.so
            _ => PathBuf::from(&self.project.module_name),
        }
    }

    pub(crate) fn add_external_libs(
        &self,
        writer: &mut VirtualWriter<WheelWriter>,
        audited: &[AuditedArtifact],
        use_shim: bool,
    ) -> Result<()> {
        if self.project.editable {
            if let Some(repairer) =
                self.make_repairer(&self.python.platform_tag, self.python.interpreter.first())
            {
                return repairer.patch_editable(audited);
            }
            return Ok(());
        }
        if audited.iter().all(|a| a.external_libs.is_empty()) {
            return Ok(());
        }

        // Log which libraries need to be copied and which artifacts require them
        // before calling patchelf, so users can see this even if patchelf is missing.
        eprintln!("🔗 External shared libraries to be copied into the wheel:");
        for aa in audited {
            if aa.external_libs.is_empty() {
                continue;
            }
            eprintln!("  {} requires:", aa.artifact.path.display());
            for lib in &aa.external_libs {
                if let Some(path) = lib.realpath.as_ref() {
                    eprintln!("    {} => {}", lib.name, path.display());
                } else {
                    eprintln!("    {} => not found", lib.name);
                }
            }
        }

        match self.python.auditwheel {
            AuditWheelMode::Warn => {
                eprintln!(
                    "⚠️  Warning: Your library requires copying the above external libraries. \
                     Re-run with `--auditwheel=repair` to copy them into the wheel."
                );
                return Ok(());
            }
            AuditWheelMode::Check => {
                bail!(
                    "Your library requires copying the above external libraries. \
                     Re-run with `--auditwheel=repair` to copy them."
                );
            }
            _ => {}
        }

        let repairer = self
            .make_repairer(&self.python.platform_tag, self.python.interpreter.first())
            .context("No wheel repairer available for this platform")?;

        // Put external libs to ${distribution_name}.libs directory
        // See https://github.com/pypa/auditwheel/issues/89
        // Use the distribution name (matching auditwheel's behavior) to avoid
        // conflicts with other packages in the same namespace.
        let dist_name = self.project.metadata24.get_distribution_escaped();
        let libs_dir = repairer.libs_dir(&dist_name);

        // Merge arch_requirements from all audited artifacts (universal2 only).
        // Each artifact may have analyzed different architecture slices.
        let merged_arch_requirements: HashMap<PathBuf, HashSet<String>> = {
            let mut merged: HashMap<PathBuf, HashSet<String>> = HashMap::new();
            for aa in audited {
                for (realpath, archs) in &aa.arch_requirements {
                    merged
                        .entry(realpath.clone())
                        .or_default()
                        .extend(archs.iter().cloned());
                }
            }
            merged
        };
        let arch_requirements = if merged_arch_requirements.is_empty() {
            None
        } else {
            Some(&merged_arch_requirements)
        };

        let temp_dir = writer.temp_dir()?;
        let (grafted, libs_copied) =
            prepare_grafted_libs(audited, temp_dir.path(), arch_requirements)?;

        // For bin bindings with external deps (shim mode), the real binary
        // lives in {dist_name}.scripts/ in platlib rather than .data/scripts/.
        // This gives us a predictable relative path to the bundled libs directory.
        let artifact_dir = if use_shim {
            self.project.metadata24.get_scripts_platlib_dir()
        } else {
            self.get_artifact_dir()
        };
        repairer.patch(audited, &grafted, &libs_dir, &artifact_dir)?;

        // Add grafted libraries to the wheel
        for lib in &grafted {
            writer.add_file_force(libs_dir.join(&lib.new_name), &lib.dest_path, true)?;
        }

        log_grafted_libs(&libs_copied, &libs_dir);

        // Apply __init__.py patch for runtime DLL discovery (Windows only).
        // The patch registers the .libs/ directory via os.add_dll_directory().
        // Skip when no libraries were actually grafted (nothing to discover),
        // for bin bridge wheels (no package __init__.py to patch), and
        // root-level artifacts.
        let depth = artifact_dir.components().count();
        if !grafted.is_empty() && depth > 0 && !self.project.bridge().is_bin() {
            let libs_dir_name = libs_dir.to_string_lossy().into_owned();
            if let Some(patch) = repairer.init_py_patch(&libs_dir_name, depth) {
                let init_py_path = artifact_dir.join("__init__.py");
                writer.prepend_to(init_py_path, patch.into_bytes())?;
            }
        }

        // Generate auditwheel SBOM for the grafted libraries.
        // This mirrors Python auditwheel's behaviour of writing a CycloneDX
        // SBOM to <dist-info>/sboms/auditwheel.cdx.json that records which OS
        // packages provided the grafted shared libraries.
        #[cfg(feature = "sbom")]
        {
            let auditwheel_sbom_enabled = self
                .artifact
                .sbom
                .as_ref()
                .and_then(|c| c.auditwheel)
                .unwrap_or(true);
            if auditwheel_sbom_enabled {
                // Obtain the sysroot so whichprovides can strip cross-compilation
                // prefixes when querying the host package manager.
                let sysroot =
                    get_sysroot_path(&self.project.target).unwrap_or_else(|_| PathBuf::from("/"));
                let mut grafted_paths: Vec<PathBuf> = libs_copied.into_iter().collect();
                grafted_paths.sort();
                if let Some(sbom_json) = crate::auditwheel::sbom::create_auditwheel_sbom(
                    &self.project.metadata24.name,
                    &self.project.metadata24.version.to_string(),
                    &grafted_paths,
                    &sysroot,
                ) {
                    let sbom_path = self
                        .project
                        .metadata24
                        .get_dist_info_dir()
                        .join("sboms/auditwheel.cdx.json");
                    writer.add_bytes(&sbom_path, None, sbom_json, false)?;
                }
            }
        }

        Ok(())
    }

    /// Stage an artifact into a private directory so that:
    /// 1. `warn_missing_py_init` can safely mmap the file without risk of
    ///    concurrent modification by cargo / rust-analyzer.
    /// 2. Auditwheel repair can modify it in-place without altering the
    ///    original cargo build output.
    ///
    /// Uses `fs::rename` for an atomic move into the staging directory,
    /// then re-materialises the staged file at the original location so
    /// that users can still find the artifact at the standard cargo
    /// output path. The copy-back prefers a hard link (O(1) on ext4,
    /// xfs, btrfs, zfs, ntfs, refs, apfs, hfs+), falls back to reflink
    /// (copy-on-write) on filesystems that disallow hard links, and
    /// finally to a full `fs::copy`.
    ///
    /// When `fs::rename` fails (e.g. cross-device), falls back to
    /// reflink-or-copy directly (hard link would fail with `EXDEV` for
    /// the same reason); the concurrent-modification window is unlikely
    /// in cross-device setups.
    pub(crate) fn stage_artifact(&self, artifact: &mut BuildArtifact) -> Result<()> {
        let maturin_build = crate::compile::ensure_target_maturin_dir(&self.project.target_dir);
        let artifact_path = &artifact.path;
        let new_artifact_path = maturin_build.join(artifact_path.file_name().unwrap());
        // Remove any stale file at the destination so that `fs::rename`
        // succeeds on Windows (where rename fails if the destination
        // already exists).
        let _ = fs::remove_file(&new_artifact_path);
        if fs::rename(artifact_path, &new_artifact_path).is_ok() {
            // Rename succeeded — we now own the only copy.  Put a copy
            // back at the original location for users who expect the
            // artifact at the standard cargo output path.  Skip if a
            // new file already appeared (cargo / rust-analyzer rebuilt).
            if artifact_path.exists() {
                tracing::debug!(
                    "Skipping copy-back: {} was recreated by another process",
                    artifact_path.display()
                );
            } else if let Err(err) = link_or_copy(&new_artifact_path, artifact_path) {
                eprintln!(
                    "⚠️  Warning: failed to copy artifact back to {}: {err:#}. The staged artifact is available at {}",
                    artifact_path.display(),
                    new_artifact_path.display()
                );
            }
        } else {
            // Rename failed (cross-device).  Fall back to reflink/copy;
            // concurrent modification is unlikely in this scenario.
            reflink_or_copy(artifact_path, &new_artifact_path)?;
        }
        artifact.path = new_artifact_path.normalize()?.into_path_buf();
        Ok(())
    }
}

/// Install `from` at `to` using the fastest filesystem primitive available.
///
/// Fallback order:
/// 1. `fs::hard_link` — O(1) on ext4, xfs, btrfs, zfs, ntfs, refs, apfs,
///    hfs+. Fails with `EXDEV` across mount points and with `EPERM`/other
///    errors on filesystems that don't support hard links (fat/exfat,
///    some smb shares).
/// 2. Reflink (`ioctl_ficlone` / `clonefile`) — O(1) on CoW filesystems
///    (btrfs, xfs with `reflink=1`, apfs). On Linux, `ioctl_ficlone`
///    does not copy metadata, so `reflink_with_permissions` fixes the
///    mode bits.
/// 3. `fs::copy` — full byte copy; always available.
///
/// Caller invariants:
/// * The parent directory of `to` must exist.
/// * `to` should not already exist — both `fs::hard_link` and reflink
///   fail with `EEXIST` if it does, so an existing destination forces
///   the slow `fs::copy` fallback (which overwrites). Both in-tree
///   callers (`stage_artifact` and the editable install) remove the
///   destination explicitly before calling this helper, both to
///   guarantee the fast path and — in the editable case — to avoid
///   SIGSEGV in processes that have the previous build `dlopen`ed
///   (see PyO3/maturin#758).
pub(crate) fn link_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    match fs::hard_link(from, to) {
        Ok(()) => {
            tracing::debug!(
                "Installed {} -> {} via hard link",
                from.display(),
                to.display()
            );
            Ok(())
        }
        Err(err) => {
            tracing::debug!(
                "Hard link {} -> {} failed ({err}); falling back to reflink/copy",
                from.display(),
                to.display()
            );
            reflink_or_copy(from, to)
        }
    }
}

/// Reflink (copy-on-write) a file, preserving permissions, and fall back to
/// a regular copy if reflink fails for any reason.
///
/// On macOS `clonefile` preserves all metadata natively.  On Linux
/// `ioctl_ficlone` only clones data blocks so we must copy permissions
/// ourselves.
///
/// Adapted from uv's `reflink_with_permissions` implementation:
/// <https://github.com/astral-sh/uv/blob/main/crates/uv-fs/src/link.rs>
/// See also: <https://github.com/astral-sh/uv/issues/18181>
fn reflink_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    if reflink_with_permissions(from, to).is_err() {
        fs::copy(from, to)?;
    }
    Ok(())
}

/// Attempt a reflink while preserving the source file's permissions.
///
/// On Linux, `ioctl_ficlone` does not copy metadata, so we reflink first
/// then copy permissions from the source to the destination.
/// On other platforms we delegate to `reflink_copy::reflink` which preserves
/// metadata natively (macOS `clonefile`).
///
/// Based on uv's approach which uses `rustix::fs::ioctl_ficlone` directly
/// with `fchmod` on the open file descriptor to avoid TOCTOU races.  We
/// simplify here by calling `reflink_copy::reflink` followed by
/// `set_permissions`, since the staged artifact lives in a private
/// directory where TOCTOU is not a concern.
/// <https://github.com/astral-sh/uv/blob/main/crates/uv-fs/src/link.rs>
#[cfg(target_os = "linux")]
fn reflink_with_permissions(from: &Path, to: &Path) -> std::io::Result<()> {
    reflink_copy::reflink(from, to)?;
    let perms = fs::metadata(from)?.permissions();
    fs::set_permissions(to, perms)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn reflink_with_permissions(from: &Path, to: &Path) -> std::io::Result<()> {
    reflink_copy::reflink(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_or_copy_installs_file_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src.bin");
        let dst = tmp.path().join("dst.bin");
        fs::write(&src, b"hello").unwrap();

        link_or_copy(&src, &dst).unwrap();

        assert_eq!(fs::read(&dst).unwrap(), b"hello");
    }

    #[cfg(unix)]
    #[test]
    fn link_or_copy_hardlinks_on_same_device() {
        use std::os::unix::fs::MetadataExt as _;

        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src.bin");
        let dst = tmp.path().join("dst.bin");
        fs::write(&src, b"hello").unwrap();

        link_or_copy(&src, &dst).unwrap();

        assert_eq!(
            fs::metadata(&src).unwrap().ino(),
            fs::metadata(&dst).unwrap().ino(),
            "link_or_copy should hard-link when source and destination are on the same filesystem",
        );
    }

    #[test]
    fn link_or_copy_errors_when_source_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("missing.bin");
        let dst = tmp.path().join("dst.bin");

        assert!(link_or_copy(&src, &dst).is_err());
    }
}
