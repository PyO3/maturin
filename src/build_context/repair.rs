#[cfg(feature = "sbom")]
use crate::auditwheel::get_sysroot_path;
use crate::auditwheel::{
    AuditWheelMode, PlatformTag, Policy, get_policy_and_libs, patchelf, relpath,
};
#[cfg(feature = "sbom")]
use crate::module_writer::ModuleWriter;
use crate::module_writer::WheelWriter;
use crate::util::hash_file;
use crate::{BridgeModel, BuildArtifact, PythonInterpreter, VirtualWriter};
use anyhow::{Context, Result, bail};
use fs_err as fs;
use lddtree::Library;
use normpath::PathExt;
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use super::BuildContext;

impl BuildContext {
    pub(super) fn auditwheel(
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
            && self.target.is_linux()
            && !python_interpreter.support_portable_wheels()
        {
            eprintln!(
                "🐍 Skipping auditwheel because {python_interpreter} does not support manylinux/musllinux wheels"
            );
            return Ok((Policy::default(), Vec::new()));
        }

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

        // only bin bindings allow linking to libpython, extension modules must not
        let allow_linking_libpython = self.bridge().is_bin();
        if self.bridge().is_bin() && !musllinux.is_empty() {
            return get_policy_and_libs(
                artifact,
                Some(musllinux[0]),
                &self.target,
                &self.project.manifest_path,
                allow_linking_libpython,
            );
        }

        let tag = others.first().or_else(|| musllinux.first()).copied();
        get_policy_and_libs(
            artifact,
            tag,
            &self.target,
            &self.project.manifest_path,
            allow_linking_libpython,
        )
    }

    /// Add library search paths in Cargo target directory rpath when building in editable mode
    fn add_rpath<A>(&self, artifacts: &[A]) -> Result<()>
    where
        A: Borrow<BuildArtifact>,
    {
        if self.editable && self.target.is_linux() && !artifacts.is_empty() {
            for artifact in artifacts {
                let artifact = artifact.borrow();
                if artifact.linked_paths.is_empty() {
                    continue;
                }
                let old_rpaths = patchelf::get_rpath(&artifact.path)?;
                let mut new_rpaths = old_rpaths.clone();
                for path in &artifact.linked_paths {
                    if !old_rpaths.contains(path) {
                        new_rpaths.push(path.to_string());
                    }
                }
                let new_rpath = new_rpaths.join(":");
                if let Err(err) = patchelf::set_rpath(&artifact.path, &new_rpath) {
                    eprintln!(
                        "⚠️ Warning: Failed to set rpath for {}: {}",
                        artifact.path.display(),
                        err
                    );
                }
            }
        }
        Ok(())
    }

    pub(super) fn add_external_libs<A>(
        &self,
        writer: &mut VirtualWriter<WheelWriter>,
        artifacts: &[A],
        ext_libs: &[Vec<Library>],
    ) -> Result<()>
    where
        A: Borrow<BuildArtifact>,
    {
        if self.editable {
            return self.add_rpath(artifacts);
        }
        if ext_libs.iter().all(|libs| libs.is_empty()) {
            return Ok(());
        }

        // Log which libraries need to be copied and which artifacts require them
        // before calling patchelf, so users can see this even if patchelf is missing.
        eprintln!("🔗 External shared libraries to be copied into the wheel:");
        for (artifact, artifact_ext_libs) in artifacts.iter().zip(ext_libs) {
            let artifact = artifact.borrow();
            if artifact_ext_libs.is_empty() {
                continue;
            }
            eprintln!("  {} requires:", artifact.path.display());
            for lib in artifact_ext_libs {
                if let Some(path) = lib.realpath.as_ref() {
                    eprintln!("    {} => {}", lib.name, path.display());
                } else {
                    eprintln!("    {} => not found", lib.name);
                }
            }
        }

        if matches!(self.python.auditwheel, AuditWheelMode::Check) {
            bail!(
                "Your library is not manylinux/musllinux compliant because it requires copying the above libraries. \
                 Re-run with `--auditwheel=repair` to copy them."
            );
        }

        patchelf::verify_patchelf()?;

        // Put external libs to ${distribution_name}.libs directory
        // See https://github.com/pypa/auditwheel/issues/89
        // Use the distribution name (matching auditwheel's behavior) to avoid
        // conflicts with other packages in the same namespace.
        let libs_dir = PathBuf::from(format!(
            "{}.libs",
            self.project.metadata24.get_distribution_escaped()
        ));

        let temp_dir = writer.temp_dir()?;
        let mut soname_map = BTreeMap::new();
        let mut libs_copied = HashSet::new();
        for lib in ext_libs.iter().flatten() {
            let lib_path = lib.realpath.clone().with_context(|| {
                format!(
                    "Cannot repair wheel, because required library {} could not be located.",
                    lib.path.display()
                )
            })?;
            // Generate a new soname with a short hash
            let short_hash = &hash_file(&lib_path)?[..8];
            let (file_stem, file_ext) = lib.name.split_once('.').with_context(|| {
                format!("Unexpected library name without extension: {}", lib.name)
            })?;
            let new_soname = if !file_stem.ends_with(&format!("-{short_hash}")) {
                format!("{file_stem}-{short_hash}.{file_ext}")
            } else {
                format!("{file_stem}.{file_ext}")
            };

            // Copy the original lib to a tmpdir and modify some of its properties
            // for example soname and rpath
            let dest_path = temp_dir.path().join(&new_soname);
            fs::copy(&lib_path, &dest_path)?;
            libs_copied.insert(lib_path);

            // fs::copy copies permissions as well, and the original
            // file may have been read-only
            let mut perms = fs::metadata(&dest_path)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            fs::set_permissions(&dest_path, perms)?;

            patchelf::set_soname(&dest_path, &new_soname)?;
            if !lib.rpath.is_empty() {
                patchelf::set_rpath(&dest_path, &libs_dir)?;
            }
            soname_map.insert(
                lib.name.clone(),
                (new_soname.clone(), dest_path.clone(), lib.needed.clone()),
            );
        }

        for (artifact, artifact_ext_libs) in artifacts.iter().zip(ext_libs) {
            let artifact = artifact.borrow();
            let artifact_deps: HashSet<_> = artifact_ext_libs.iter().map(|lib| &lib.name).collect();
            let replacements = soname_map
                .iter()
                .filter_map(|(k, v)| {
                    if artifact_deps.contains(k) {
                        Some((k, v.0.clone()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if !replacements.is_empty() {
                patchelf::replace_needed(&artifact.path, &replacements[..])?;
            }
        }

        // we grafted in a bunch of libraries and modified their sonames, but
        // they may have internal dependencies (DT_NEEDED) on one another, so
        // we need to update those records so each now knows about the new
        // name of the other.
        for (new_soname, path, needed) in soname_map.values() {
            let mut replacements = Vec::new();
            for n in needed {
                if soname_map.contains_key(n) {
                    replacements.push((n, soname_map[n].0.clone()));
                }
            }
            if !replacements.is_empty() {
                patchelf::replace_needed(path, &replacements[..])?;
            }
            // Use add_file_force to bypass exclusion checks for external shared libraries
            writer.add_file_force(libs_dir.join(new_soname), path, true)?;
        }

        // Sort for deterministic output.
        let mut grafted_paths: Vec<PathBuf> = libs_copied.into_iter().collect();
        grafted_paths.sort();

        eprintln!(
            "🖨  Copied external shared libraries to package {} directory.",
            libs_dir.display()
        );

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
                let sysroot = get_sysroot_path(&self.target).unwrap_or_else(|_| PathBuf::from("/"));
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

        let artifact_dir = match self.bridge() {
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
        };
        for artifact in artifacts {
            let artifact = artifact.borrow();
            let mut new_rpaths = patchelf::get_rpath(&artifact.path)?;
            // TODO: clean existing rpath entries if it's not pointed to a location within the wheel
            // See https://github.com/pypa/auditwheel/blob/353c24250d66951d5ac7e60b97471a6da76c123f/src/auditwheel/repair.py#L160
            let new_rpath = Path::new("$ORIGIN").join(relpath(&libs_dir, &artifact_dir));
            new_rpaths.push(new_rpath.to_str().unwrap().to_string());
            let new_rpath = new_rpaths.join(":");
            patchelf::set_rpath(&artifact.path, &new_rpath)?;
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
    /// Scenario.
    pub(super) fn stage_artifact(&self, artifact: &mut BuildArtifact) -> Result<()> {
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
