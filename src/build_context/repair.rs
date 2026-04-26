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

    /// Patch the audited artifacts to bundle their external shared library
    /// dependencies into the wheel.
    ///
    /// Returns `true` if any artifact was patched in place by patchelf /
    /// patch_macho / pe_patch, `false` otherwise. The caller uses this to
    /// decide whether to restore the staged artifact to the cargo output
    /// path post-wheel-write — see [`finalize_staged_artifacts`] and #3111.
    pub(crate) fn add_external_libs(
        &self,
        writer: &mut VirtualWriter<WheelWriter>,
        audited: &[AuditedArtifact],
        use_shim: bool,
    ) -> Result<bool> {
        if self.project.editable {
            if let Some(repairer) =
                self.make_repairer(&self.python.platform_tag, self.python.interpreter.first())
            {
                return repairer.patch_editable(audited);
            }
            return Ok(false);
        }
        if audited.iter().all(|a| a.external_libs.is_empty()) {
            return Ok(false);
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
                // Warn mode does not modify the artifact.
                return Ok(false);
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

        Ok(true)
    }

    /// Stage an artifact into a private directory so that:
    /// 1. `warn_missing_py_init` can safely mmap the file without risk of
    ///    concurrent modification by cargo / rust-analyzer.
    /// 2. Auditwheel repair can modify it in-place without altering the
    ///    original cargo build output (see #2969 / #2680).
    ///
    /// `fs::rename` is atomic on the same filesystem; on cross-device it
    /// falls back to `reflink_or_copy` + `fs::remove_file`. The original
    /// cargo output path is recorded on the artifact so it can be
    /// restored after the wheel is written via [`finalize_staged_artifacts`]
    /// when no auditwheel patching occurred — see #3111.
    ///
    /// Tradeoffs vs. the previous "copy back immediately" behavior:
    ///
    /// - **Cross-device:** the unpatched/finalize path does
    ///   `reflink_or_copy` + `remove_file` here and again in
    ///   [`finalize_staged_artifacts`], i.e. two full copies instead of one.
    ///   This is the expected cost of moving the cargo-path restore to
    ///   after the wheel is written, and only matters when the cargo
    ///   target dir and the maturin staging dir are on different
    ///   filesystems — uncommon in practice.
    /// - **Build errors before finalize:** if the build errors after
    ///   `stage_artifact` but before [`finalize_staged_artifacts`] runs
    ///   (e.g. `auditwheel`, `writer.finish`, or any step in between
    ///   fails), the cargo output path stays absent. The wheel is the
    ///   deliverable so this is not a correctness bug, and the staged
    ///   artifact remains recoverable from `target/maturin/`. Cargo will
    ///   recompile on the next build — which it had to do anyway because
    ///   the source-of-truth file moved.
    pub(crate) fn stage_artifact(&self, artifact: &mut BuildArtifact) -> Result<()> {
        let maturin_build = crate::compile::ensure_target_maturin_dir(&self.project.target_dir);
        let cargo_output = artifact.path.clone();
        artifact.path = stage_file(&cargo_output, &maturin_build)?;
        artifact.cargo_output_path = Some(cargo_output);
        Ok(())
    }
}

/// Move `artifact_path` into `staging_dir` and return the new normalized
/// path. Uses `fs::rename` (atomic on the same filesystem) and falls back
/// to `reflink_or_copy` + `fs::remove_file` on cross-device. After this
/// returns, `artifact_path` no longer exists.
fn stage_file(artifact_path: &Path, staging_dir: &Path) -> Result<PathBuf> {
    let new_path = staging_dir.join(artifact_path.file_name().unwrap());
    // Remove any stale file at the destination so that `fs::rename`
    // succeeds on Windows (where rename fails if the destination
    // already exists).
    let _ = fs::remove_file(&new_path);
    if fs::rename(artifact_path, &new_path).is_err() {
        // Cross-device. Reflink/copy and remove the original so the
        // post-wheel-write finalize step doesn't see a stale unpatched
        // file there.
        reflink_or_copy(artifact_path, &new_path)?;
        let _ = fs::remove_file(artifact_path);
    }
    Ok(new_path.normalize()?.into_path_buf())
}

/// Restore each staged artifact to its original cargo output path after a
/// successful wheel write — but only when no auditwheel patching occurred.
///
/// When patches were applied, the staged artifact has rewritten `DT_NEEDED`
/// / Mach-O load commands / PE imports that don't match cargo's view of the
/// world; restoring it would re-introduce the bug fixed by #2969 / #2680
/// (patched bytes confusing cargo's incremental cache and lddtree on a
/// subsequent build). In that case the cargo output path is left empty —
/// cargo will recompile on the next build, which it had to do anyway.
///
/// `was_patched` is build-wide rather than per-artifact: in a multi-bin
/// project where some bins were patched and others weren't, the unpatched
/// bins are also skipped from rename-back and will be recompiled by cargo
/// next time. This is intentional — the only call site that can produce
/// multiple artifacts is `build_bin_wheel`, and a project with multiple
/// `[[bin]]` targets where some bundle external libs and others don't is
/// rare enough that per-artifact tracking isn't worth the complexity.
///
/// Failures are logged and swallowed: the wheel is the deliverable, the
/// cargo-path restore is a UX convenience, and the staged artifact is
/// still recoverable from `target/maturin/`.
pub(crate) fn finalize_staged_artifacts(audited: &[AuditedArtifact], was_patched: bool) {
    if was_patched {
        return;
    }
    for aa in audited {
        let Some(cargo_output) = aa.artifact.cargo_output_path.as_deref() else {
            continue;
        };
        if let Err(err) = finalize_staged_artifact(&aa.artifact.path, cargo_output) {
            tracing::warn!(
                "Could not restore artifact to cargo output path {}: {err:#}",
                cargo_output.display()
            );
        }
    }
}

/// Restore a single staged artifact to the cargo output path. Must only be
/// called when the staged artifact has not been modified by auditwheel /
/// patchelf / patch_macho / pe_patch — see #2969.
///
/// O(1) `fs::rename` on every mainstream filesystem (ext4, xfs, btrfs,
/// zfs, ntfs, refs, apfs, hfs+). On cross-device, falls back to
/// `reflink_or_copy` + `fs::remove_file`.
fn finalize_staged_artifact(staged: &Path, cargo_output: &Path) -> Result<()> {
    // `fs::rename` overwrites the destination on Unix; on Windows it
    // fails if the destination already exists, so remove anything that
    // might be there from a prior partial build.
    let _ = fs::remove_file(cargo_output);
    if fs::rename(staged, cargo_output).is_ok() {
        tracing::debug!(
            "Restored {} to {}",
            staged.display(),
            cargo_output.display()
        );
        return Ok(());
    }
    // Cross-device or permission error: fall back to reflink/copy and
    // remove the staged file so it doesn't accumulate in target/maturin.
    reflink_or_copy(staged, cargo_output).with_context(|| {
        format!(
            "Failed to restore staged artifact from {} to {}",
            staged.display(),
            cargo_output.display(),
        )
    })?;
    let _ = fs::remove_file(staged);
    Ok(())
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
