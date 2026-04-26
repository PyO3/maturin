#[cfg(feature = "auditwheel")]
use crate::auditwheel::MacOSRepairer;
#[cfg(feature = "auditwheel")]
use crate::auditwheel::WindowsRepairer;
#[cfg(feature = "sbom")]
use crate::auditwheel::get_sysroot_path;
use crate::auditwheel::{
    AuditResult, AuditWheelMode, AuditedArtifact, ElfRepairer, PatchKind, PlatformTag, Policy,
    WheelRepairer, log_grafted_libs, prepare_grafted_libs,
};
use crate::compile::CargoOutputState;
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

    /// Patch the audited artifacts to bundle their external shared library
    /// dependencies into the wheel.
    ///
    /// Before any in-place patching runs, [`copy_back_cargo_outputs`]
    /// reflinks/copies a clean copy of every artifact that the upcoming
    /// [`WheelRepairer::patch`] call will rewrite back to its cargo output
    /// path and transitions its [`CargoOutputState`] to `Patched`, so
    /// [`finalize_staged_artifacts`] knows not to overwrite the unpatched
    /// bytes with the patched ones (see #2969 / #3111). Artifacts that
    /// will not be patched keep their `Renamed` / `Mirrored` state and
    /// get the cheap rename-back in finalize as usual.
    pub(crate) fn add_external_libs(
        &self,
        writer: &mut VirtualWriter<WheelWriter>,
        audited: &mut [AuditedArtifact],
        use_shim: bool,
    ) -> Result<()> {
        if self.project.editable {
            if let Some(repairer) =
                self.make_repairer(&self.python.platform_tag, self.python.interpreter.first())
            {
                let kind = PatchKind::Editable;
                let will_patch = repairer.patch_required(audited, &kind);
                copy_back_cargo_outputs(audited, &will_patch)?;
                return repairer.patch(audited, &kind);
            }
            return Ok(());
        }
        if audited.iter().all(|a| a.external_libs.is_empty()) {
            return Ok(());
        }

        // Log which libraries need to be copied and which artifacts require them
        // before calling patchelf, so users can see this even if patchelf is missing.
        eprintln!("🔗 External shared libraries to be copied into the wheel:");
        for aa in audited.iter() {
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
            for aa in audited.iter() {
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
        // Reflink/copy a clean unpatched copy of every artifact that the
        // upcoming `repairer.patch` call will rewrite back to its cargo
        // output path before any bytes are touched, then transition its
        // staging state to `Patched` so finalize doesn't try to overwrite
        // with the patched bytes — see #2969.
        let kind = PatchKind::Repair {
            grafted: &grafted,
            libs_dir: &libs_dir,
            artifact_dir: &artifact_dir,
        };
        let will_patch = repairer.patch_required(audited, &kind);
        copy_back_cargo_outputs(audited, &will_patch)?;
        repairer.patch(audited, &kind)?;

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
    ///    original cargo build output (see #2969 / #2680).
    ///
    /// On the same filesystem `fs::rename` is atomic and the cargo output
    /// path becomes empty until [`finalize_staged_artifacts`] renames it
    /// back. On cross-device the file is reflinked/copied into staging and
    /// the original is left in place — finalize then just drops the staged
    /// duplicate, avoiding the second full copy that the previous
    /// implementation performed.
    ///
    /// The post-stage state is recorded on the artifact as a
    /// [`CargoOutputState`] (`Renamed` / `Mirrored`) so it can be restored
    /// either by [`finalize_staged_artifacts`] (cheap rename or duplicate
    /// drop, when no patching happened) or transitioned to `Patched` by
    /// [`copy_back_cargo_outputs`] (which puts a clean unpatched copy at
    /// the cargo output path before the repairer rewrites bytes in place
    /// — see #2969 / #3111).
    ///
    /// **Build errors before finalize:** if the build errors after
    /// `stage_artifact` but before [`finalize_staged_artifacts`] runs, the
    /// staged artifact stays in `target/maturin/`. The wheel is the
    /// deliverable so this is not a correctness bug, and on the next
    /// compile cargo will write a fresh artifact at the cargo output path.
    pub(crate) fn stage_artifact(&self, artifact: &mut BuildArtifact) -> Result<()> {
        let maturin_build = crate::compile::ensure_target_maturin_dir(&self.project.target_dir);
        let cargo_output = artifact.path.clone();
        let (staged_path, staging) = stage_file(&cargo_output, &maturin_build)?;
        artifact.path = staged_path;
        artifact.staging = staging;
        Ok(())
    }
}

/// Place a copy of `artifact_path` into `staging_dir` and return the new
/// normalized path along with the [`CargoOutputState`] describing how the
/// move was performed.
///
/// Same-filesystem fast path (returns [`CargoOutputState::Renamed`]):
/// `fs::rename` atomically moves the file (and overwrites any stale
/// leftover from a previous build on both Unix and Windows); the cargo
/// output path is empty until [`finalize_staged_artifacts`] renames it
/// back.
///
/// Cross-device fallback (returns [`CargoOutputState::Mirrored`]):
/// reflink/copy into staging and **leave the original at the cargo output
/// path**; finalize will see the typed `Mirrored` state and drop the
/// staged duplicate, so the bytes are only copied once. The pre-clean is
/// needed here because `reflink_copy::reflink` refuses to overwrite on
/// some platforms.
///
/// Only `ErrorKind::CrossesDevices` triggers the fallback — every other
/// rename error (permission denied, I/O failure, etc.) is surfaced
/// instead of being silently masked by the copy path.
fn stage_file(artifact_path: &Path, staging_dir: &Path) -> Result<(PathBuf, CargoOutputState)> {
    let new_path = staging_dir.join(artifact_path.file_name().unwrap());
    let staging = match fs::rename(artifact_path, &new_path) {
        Ok(()) => CargoOutputState::Renamed {
            cargo_output: artifact_path.to_path_buf(),
        },
        Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
            // Cross-device: copy into staging and leave the original alone.
            let _ = fs::remove_file(&new_path);
            reflink_or_copy(artifact_path, &new_path)?;
            CargoOutputState::Mirrored {
                cargo_output: artifact_path.to_path_buf(),
            }
        }
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "Failed to stage {} -> {}",
                    artifact_path.display(),
                    new_path.display(),
                )
            });
        }
    };
    Ok((new_path.normalize()?.into_path_buf(), staging))
}

/// Copy the clean unpatched bytes back to every artifact's cargo output
/// path **before** the platform-specific repairer rewrites bytes in place,
/// then transition the artifact's [`CargoOutputState`] to `Patched` so
/// [`finalize_staged_artifacts`] doesn't try to overwrite the unpatched
/// bytes with the patched ones.
///
/// `will_patch[i] == false` artifacts are left alone — they keep their
/// `Renamed` / `Mirrored` state and get the cheap rename-back (or
/// duplicate drop) in finalize.
///
/// For artifacts marked `will_patch[i] == true`:
///
/// - **Mirrored** ([`stage_file`] was cross-device): the cargo output
///   path already holds the unpatched original, so we only flip the
///   state — no I/O needed.
/// - **Renamed** ([`stage_file`] used `fs::rename`): the cargo output
///   path is empty; reflink/copy the still-clean staged artifact back
///   to it before the repairer rewrites bytes in place. This is one
///   reflink (O(1) on apfs/btrfs/xfs/refs, a real copy on ext4) per
///   patched artifact, scoped only to artifacts that genuinely need
///   patching — multi-bin projects where some bins are unpatched still
///   pay only the rename-back for those bins.
/// - **NotStaged** / **Patched**: no-op. `NotStaged` shouldn't happen on
///   an audited artifact (we always stage before auditing) but we
///   degrade gracefully; `Patched` means a previous call already did
///   the work.
fn copy_back_cargo_outputs(audited: &mut [AuditedArtifact], will_patch: &[bool]) -> Result<()> {
    for (aa, &will_patch) in audited.iter_mut().zip(will_patch) {
        if !will_patch {
            continue;
        }
        match &aa.artifact.staging {
            CargoOutputState::Renamed { cargo_output } => {
                let cargo_output = cargo_output.clone();
                reflink_or_copy(&aa.artifact.path, &cargo_output).with_context(|| {
                    format!(
                        "Failed to restore unpatched cargo output {} from staged artifact {}",
                        cargo_output.display(),
                        aa.artifact.path.display(),
                    )
                })?;
                tracing::debug!(
                    "copy_back_cargo_outputs: copied unpatched {} -> {}",
                    aa.artifact.path.display(),
                    cargo_output.display()
                );
                aa.artifact.staging = CargoOutputState::Patched;
            }
            CargoOutputState::Mirrored { cargo_output } => {
                tracing::debug!(
                    "copy_back_cargo_outputs: {} already holds unpatched original (cross-device staging), no-op",
                    cargo_output.display()
                );
                aa.artifact.staging = CargoOutputState::Patched;
            }
            CargoOutputState::NotStaged | CargoOutputState::Patched => {}
        }
    }
    Ok(())
}

/// Restore staged artifacts to their cargo output paths after a successful
/// wheel write.
///
/// Dispatches on the artifact's [`CargoOutputState`] (set deterministically
/// by [`stage_file`] / [`copy_back_cargo_outputs`]) so the staging mode is
/// never re-inferred from `cargo_output.exists()`:
///
/// - `NotStaged`: no staging happened, nothing to do.
/// - `Renamed { cargo_output }`: rename the staged file back to
///   `cargo_output`. If rename fails (extremely rare — e.g. `target/`
///   was relocated to a different mount point mid-build), fall back to
///   reflink+remove.
/// - `Mirrored { cargo_output }`: `cargo_output` already holds the
///   unpatched original from the cross-device branch of `stage_file`;
///   drop the staged duplicate.
/// - `Patched`: [`copy_back_cargo_outputs`] already restored unpatched
///   bytes at the cargo output path; the staged artifact (which now
///   holds patched bytes) must NOT be moved back. Leave the staged
///   file in `target/maturin/` (overwritten on the next build) — see
///   #2969 for why patched bytes must not flow back to cargo's
///   incremental cache.
///
/// Per-artifact precision means a multi-bin project where only some
/// bins bundle external libraries still gets the cheap rename-back for
/// the unpatched bins.
///
/// Failures are logged and swallowed: the wheel is the deliverable, the
/// cargo-path restore is a UX convenience, and the staged artifact is
/// still recoverable from `target/maturin/`.
pub(crate) fn finalize_staged_artifacts(audited: &[AuditedArtifact]) {
    for aa in audited {
        match &aa.artifact.staging {
            CargoOutputState::NotStaged | CargoOutputState::Patched => continue,
            CargoOutputState::Renamed { cargo_output } => {
                if let Err(err) = restore_renamed(&aa.artifact.path, cargo_output) {
                    tracing::warn!(
                        "Could not restore artifact to cargo output path {}: {err:#}",
                        cargo_output.display()
                    );
                }
            }
            CargoOutputState::Mirrored { cargo_output } => {
                let _ = fs::remove_file(&aa.artifact.path);
                tracing::debug!(
                    "Dropped staged duplicate of cross-device artifact at {}",
                    cargo_output.display()
                );
            }
        }
    }
}

/// Restore a `Renamed` staged artifact back to its cargo output path.
///
/// Must only be called when the staged artifact has not been modified by
/// auditwheel / patchelf / patch_macho / pe_patch — see #2969. The typed
/// `Renamed` state guarantees that: `copy_back_cargo_outputs` transitions
/// to `Patched` *before* the repairer rewrites bytes.
///
/// 1. Same-filesystem: O(1) `fs::rename` puts the staged file back.
/// 2. Fallback (rare — e.g. `target/` straddles a mount point added
///    after staging): `reflink_or_copy` + `remove_file`.
fn restore_renamed(staged: &Path, cargo_output: &Path) -> Result<()> {
    if fs::rename(staged, cargo_output).is_ok() {
        tracing::debug!(
            "Restored {} to {}",
            staged.display(),
            cargo_output.display()
        );
        return Ok(());
    }
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
