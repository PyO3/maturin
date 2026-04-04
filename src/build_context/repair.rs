#[cfg(feature = "auditwheel")]
use crate::auditwheel::MacOSRepairer;
#[cfg(feature = "sbom")]
use crate::auditwheel::get_sysroot_path;
use crate::auditwheel::{
    AuditWheelMode, AuditedArtifact, ElfRepairer, PlatformTag, Policy, WheelRepairer,
    log_grafted_libs, prepare_grafted_libs,
};
#[cfg(feature = "sbom")]
use crate::module_writer::ModuleWriter;
use crate::module_writer::WheelWriter;
use crate::{BridgeModel, BuildArtifact, PythonInterpreter, VirtualWriter};
use anyhow::{Context, Result, bail};
use fs_err as fs;
use lddtree::Library;
use normpath::PathExt;
use std::path::{Path, PathBuf};

use super::BuildContext;

impl BuildContext {
    /// Create the appropriate platform-specific wheel repairer.
    fn make_repairer(&self, platform_tag: &[PlatformTag]) -> Option<Box<dyn WheelRepairer>> {
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
        } else {
            None
        }
    }

    pub(crate) fn auditwheel(
        &self,
        artifact: &BuildArtifact,
        platform_tag: &[PlatformTag],
        python_interpreter: Option<&PythonInterpreter>,
    ) -> Result<(Policy, Vec<Library>)> {
        if matches!(self.python.auditwheel, AuditWheelMode::Skip) {
            return Ok((Policy::default(), Vec::new()));
        }

        if let Some(python_interpreter) = python_interpreter
            && platform_tag.is_empty()
            && self.project.target.is_linux()
            && !python_interpreter.support_portable_wheels()
        {
            eprintln!(
                "🐍 Skipping auditwheel because {python_interpreter} does not support manylinux/musllinux wheels"
            );
            return Ok((Policy::default(), Vec::new()));
        }

        let repairer = match self.make_repairer(platform_tag) {
            Some(r) => r,
            None => return Ok((Policy::default(), Vec::new())),
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
    ) -> Result<()> {
        if self.project.editable {
            if let Some(repairer) = self.make_repairer(&self.python.platform_tag) {
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

        if matches!(self.python.auditwheel, AuditWheelMode::Check) {
            bail!(
                "Your library requires copying the above external libraries. \
                 Re-run with `--auditwheel=repair` to copy them."
            );
        }

        let repairer = self
            .make_repairer(&self.python.platform_tag)
            .context("No wheel repairer available for this platform")?;

        // Put external libs to ${distribution_name}.libs directory
        // See https://github.com/pypa/auditwheel/issues/89
        // Use the distribution name (matching auditwheel's behavior) to avoid
        // conflicts with other packages in the same namespace.
        let dist_name = self.project.metadata24.get_distribution_escaped();
        let libs_dir = repairer.libs_dir(&dist_name);

        let temp_dir = writer.temp_dir()?;
        let (grafted, libs_copied) = prepare_grafted_libs(audited, temp_dir.path())?;

        let artifact_dir = self.get_artifact_dir();
        repairer.patch(audited, &grafted, &libs_dir, &artifact_dir)?;

        // Add grafted libraries to the wheel
        for lib in &grafted {
            writer.add_file_force(libs_dir.join(&lib.new_name), &lib.dest_path, true)?;
        }

        log_grafted_libs(&libs_copied, &libs_dir);

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
    /// then copies the staged file back to the original location so that
    /// users can still find the artifact at the standard cargo output
    /// path. The copy-back uses reflink (copy-on-write) when available
    /// for near-instant, zero-cost copies, and falls back to a regular
    /// `fs::copy` otherwise.
    ///
    /// When `fs::rename` fails (e.g. cross-device), falls back to
    /// reflink-or-copy directly; the concurrent-modification window is
    /// unlikely in cross-device setups.
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
            } else if let Err(err) = reflink_or_copy(&new_artifact_path, artifact_path) {
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
fn reflink_or_copy(from: &Path, to: &Path) -> Result<()> {
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
