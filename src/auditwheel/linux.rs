//! Linux/ELF wheel audit and repair.
//!
//! This module implements [`WheelRepairer`] for Linux ELF binaries,
//! providing the Rust equivalent of [auditwheel](https://github.com/pypa/auditwheel).
//!
//! Delegates to the ELF compliance audit in [`super::audit`] and uses
//! `patchelf` for binary patching (SONAME, DT_NEEDED, RPATH).

use super::audit::{
    AuditWheelError, IS_LIBPYTHON, VersionedLibrary, auditwheel_rs, find_versioned_libraries,
    get_default_platform_policies, get_sysroot_path, is_dynamic_linker, relpath,
};
use super::policy::{MANYLINUX_POLICIES, MUSLLINUX_POLICIES};
use super::repair::{GraftedLib, WheelRepairer};
use super::{PlatformTag, Policy, patchelf};
use crate::compile::BuildArtifact;
use crate::target::Target;
use anyhow::{Context, Result, bail};
use goblin::elf::Elf;
use lddtree::Library;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Linux/ELF wheel repairer (auditwheel equivalent).
///
/// Bundles external `.so` files and rewrites ELF metadata (SONAME, DT_NEEDED,
/// RPATH) using `patchelf` so that `$ORIGIN`-relative references resolve to
/// the bundled copies in the `.libs/` directory.
///
/// Unlike the macOS repairer, `audit()` performs full
/// manylinux/musllinux compliance checking — the returned [`Policy`]
/// determines which `manylinux_X_Y` / `musllinux_X_Y` platform tag the wheel
/// qualifies for.
pub struct ElfRepairer {
    /// The requested platform tag (e.g., manylinux_2_17), if any.
    pub platform_tag: Option<PlatformTag>,
    /// The build target (architecture + OS).
    pub target: Target,
    /// Path to the project's Cargo.toml (used to extract RUSTFLAGS library paths).
    pub manifest_path: PathBuf,
    /// Whether the artifact is allowed to link libpython (bin bindings only).
    pub allow_linking_libpython: bool,
}

impl WheelRepairer for ElfRepairer {
    fn audit(
        &self,
        artifact: &BuildArtifact,
        _ld_paths: Vec<PathBuf>,
    ) -> Result<(Policy, Vec<Library>)> {
        get_policy_and_libs(
            artifact,
            self.platform_tag,
            &self.target,
            &self.manifest_path,
            self.allow_linking_libpython,
        )
    }

    fn patch(
        &self,
        artifacts: &[&BuildArtifact],
        grafted: &[GraftedLib],
        libs_dir: &Path,
        artifact_dir: &Path,
    ) -> Result<()> {
        patchelf::verify_patchelf()?;

        // Build a lookup from original name → new soname for rewriting references.
        let mut name_map: BTreeMap<&str, &str> = BTreeMap::new();
        for l in grafted {
            name_map.insert(l.original_name.as_str(), l.new_name.as_str());
            for alias in &l.aliases {
                name_map.insert(alias.as_str(), l.new_name.as_str());
            }
        }

        // Set soname and rpath on each grafted library.
        for lib in grafted {
            patchelf::set_soname(&lib.dest_path, &lib.new_name)?;
            if !lib.rpath.is_empty() {
                patchelf::set_rpath(&lib.dest_path, &"$ORIGIN".to_string())?;
            }
        }

        // Rewrite DT_NEEDED in each artifact to reference new sonames.
        let replacements: Vec<_> = name_map.iter().map(|(k, v)| (*k, v.to_string())).collect();
        for artifact in artifacts {
            if !replacements.is_empty() {
                patchelf::replace_needed(&artifact.path, &replacements)?;
            }
        }

        // Update cross-references between grafted libraries
        for lib in grafted {
            let lib_replacements: Vec<_> = lib
                .needed
                .iter()
                .filter_map(|n| {
                    name_map
                        .get(n.as_str())
                        .map(|new| (n.as_str(), new.to_string()))
                })
                .collect();
            if !lib_replacements.is_empty() {
                patchelf::replace_needed(&lib.dest_path, &lib_replacements)?;
            }
        }

        // Set RPATH on artifacts to find the libs directory
        for artifact in artifacts {
            let mut new_rpaths = patchelf::get_rpath(&artifact.path)?;
            let new_rpath = Path::new("$ORIGIN").join(relpath(libs_dir, artifact_dir));
            new_rpaths.push(new_rpath.to_str().unwrap().to_string());
            let new_rpath = new_rpaths.join(":");
            patchelf::set_rpath(&artifact.path, &new_rpath)?;
        }

        Ok(())
    }
}

/// Find external shared library dependencies (Linux/ELF specific).
///
/// Uses lddtree to resolve dependencies, then filters out the dynamic linker,
/// musl libc, and libraries on the policy whitelist.
#[allow(clippy::result_large_err)]
fn find_external_libs(
    artifact: impl AsRef<Path>,
    policy: &Policy,
    sysroot: PathBuf,
    ld_paths: Vec<PathBuf>,
) -> Result<Vec<Library>, AuditWheelError> {
    let dep_analyzer = lddtree::DependencyAnalyzer::new(sysroot).library_paths(ld_paths);
    let deps = dep_analyzer
        .analyze(artifact)
        .map_err(AuditWheelError::DependencyAnalysisError)?;
    let mut ext_libs = Vec::new();
    for (_, lib) in deps.libraries {
        let name = &lib.name;
        // Skip dynamic linker/loader, musl libc, and white-listed libs
        if is_dynamic_linker(name)
            || name.starts_with("libc.")
            || policy.lib_whitelist.contains(name)
        {
            continue;
        }
        ext_libs.push(lib);
    }
    Ok(ext_libs)
}

/// For the given compilation result, return the manylinux/musllinux policy and
/// the external libs we need to add to repair it.
fn get_policy_and_libs(
    artifact: &BuildArtifact,
    platform_tag: Option<PlatformTag>,
    target: &Target,
    manifest_path: &Path,
    allow_linking_libpython: bool,
) -> Result<(Policy, Vec<Library>)> {
    let (policy, should_repair) =
        auditwheel_rs(artifact, target, platform_tag, allow_linking_libpython).with_context(
            || {
                if let Some(platform_tag) = platform_tag {
                    format!("Error ensuring {platform_tag} compliance")
                } else {
                    "Error checking for manylinux/musllinux compliance".to_string()
                }
            },
        )?;
    let external_libs = if should_repair {
        let sysroot = get_sysroot_path(target).unwrap_or_else(|_| PathBuf::from("/"));
        let mut ld_paths: Vec<PathBuf> = artifact.linked_paths.iter().map(PathBuf::from).collect();

        // Add library search paths from RUSTFLAGS
        if let Some(rustflags_paths) = extract_rustflags_library_paths(manifest_path, target) {
            ld_paths.extend(rustflags_paths);
        }

        let external_libs = find_external_libs(&artifact.path, &policy, sysroot, ld_paths)
            .with_context(|| {
                if let Some(platform_tag) = platform_tag {
                    format!("Error repairing wheel for {platform_tag} compliance")
                } else {
                    "Error repairing wheel for manylinux/musllinux compliance".to_string()
                }
            })?;
        if allow_linking_libpython {
            external_libs
                .into_iter()
                .filter(|lib| !IS_LIBPYTHON.is_match(&lib.name))
                .collect()
        } else {
            external_libs
        }
    } else {
        Vec::new()
    };

    // Check external libraries for versioned symbol requirements that may
    // require a stricter (less compatible, e.g. newer manylinux) policy than what
    // the main artifact alone would need. See https://github.com/PyO3/maturin/issues/1490
    let policy = if !external_libs.is_empty() {
        let (adjusted, offenders) = check_external_libs_policy(&policy, &external_libs, target)?;
        if platform_tag.is_some() && !offenders.is_empty() {
            let tag_kind = if policy.name.starts_with("musllinux") {
                "musllinux"
            } else {
                "manylinux"
            };
            bail!(
                "External libraries {offenders:?} require newer symbol versions than {policy} allows. \
                 Consider using --compatibility {adjusted} or a newer {tag_kind} tag"
            );
        }
        adjusted
    } else {
        policy
    };

    Ok((policy, external_libs))
}

/// Return the symbol versions required by external libraries that are not
/// allowed by the given policy.
fn unsatisfied_symbol_versions(
    policy: &Policy,
    arch: &str,
    versioned_libraries: &[VersionedLibrary],
) -> Vec<String> {
    let arch_versions = match policy.symbol_versions.get(arch) {
        Some(v) => v,
        None => return vec!["(unsupported arch)".to_string()],
    };
    let mut unsatisfied = Vec::new();
    for library in versioned_libraries {
        if !policy.lib_whitelist.contains(&library.name) {
            continue;
        }
        for (name, versions_needed) in library.parsed_versions() {
            match arch_versions.get(&name) {
                Some(versions_allowed) => {
                    for v in versions_needed.difference(versions_allowed) {
                        unsatisfied.push(format!("{name}_{v}"));
                    }
                }
                None => {
                    for v in &versions_needed {
                        unsatisfied.push(format!("{name}_{v}"));
                    }
                }
            }
        }
    }
    unsatisfied.sort();
    unsatisfied
}

/// Check if external libraries require a newer glibc than the current policy allows.
/// Returns the adjusted policy and a list of descriptions for libraries that caused
/// a downgrade.
fn check_external_libs_policy(
    policy: &Policy,
    external_libs: &[Library],
    target: &Target,
) -> Result<(Policy, Vec<String>)> {
    let arch = target.target_arch().to_string();
    let mut platform_policies = if policy.name.starts_with("musllinux") {
        MUSLLINUX_POLICIES.clone()
    } else if policy.name.starts_with("manylinux") {
        MANYLINUX_POLICIES.clone()
    } else {
        get_default_platform_policies()
    };
    for p in &mut platform_policies {
        p.fixup_musl_libc_so_name(target.target_arch());
    }
    debug_assert!(
        platform_policies
            .windows(2)
            .all(|w| w[0].priority >= w[1].priority)
    );

    let mut result = policy.clone();
    let mut offenders = Vec::new();
    for lib in external_libs {
        let lib_path = match lib.realpath.as_ref() {
            Some(path) => path,
            None => continue,
        };
        let buffer = fs_err::read(lib_path)
            .with_context(|| format!("Failed to read external library {}", lib_path.display()))?;
        let elf = match Elf::parse(&buffer) {
            Ok(elf) => elf,
            Err(_) => continue,
        };
        let versioned_libraries = find_versioned_libraries(&elf);
        if versioned_libraries.is_empty() {
            continue;
        }

        let unsatisfied = unsatisfied_symbol_versions(&result, &arch, &versioned_libraries);
        if unsatisfied.is_empty() {
            continue;
        }
        for candidate in platform_policies.iter() {
            if candidate.priority > result.priority {
                continue;
            }
            if unsatisfied_symbol_versions(candidate, &arch, &versioned_libraries).is_empty() {
                if candidate.priority < result.priority {
                    debug!(
                        "Downgrading tag to {candidate} because external library {} requires {}",
                        lib.name,
                        unsatisfied.join(", "),
                    );
                    offenders.push(format!("{} ({})", lib.name, unsatisfied.join(", ")));
                    result = candidate.clone();
                }
                break;
            }
        }
    }
    Ok((result, offenders))
}

/// Extract library search paths from RUSTFLAGS configuration.
#[cfg_attr(test, allow(dead_code))]
fn extract_rustflags_library_paths(manifest_path: &Path, target: &Target) -> Option<Vec<PathBuf>> {
    let manifest_dir = manifest_path.parent()?;
    let config = cargo_config2::Config::load_with_cwd(manifest_dir).ok()?;
    let rustflags = config.rustflags(target.target_triple()).ok()??;

    let encoded = rustflags.encode().ok()?;

    let mut library_paths = Vec::new();
    for flag in rustflags::from_encoded(encoded.as_ref()) {
        if let rustflags::Flag::LibrarySearchPath { kind: _, path } = flag {
            library_paths.push(path);
        }
    }

    if library_paths.is_empty() {
        None
    } else {
        Some(library_paths)
    }
}

#[cfg(test)]
mod tests {
    use crate::Target;

    #[test]
    fn test_extract_rustflags_library_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest_path = temp_dir.path().join("Cargo.toml");
        let cargo_dir = temp_dir.path().join(".cargo");
        let config_path = cargo_dir.join("config.toml");

        fs_err::create_dir_all(&cargo_dir).unwrap();

        fs_err::write(
            &manifest_path,
            r#"
[package]
name = "test-package"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();

        fs_err::write(
            &config_path,
            r#"
[build]
rustflags = ["-L", "dependency=/usr/local/lib", "-L", "/some/other/path", "-C", "opt-level=3"]
"#,
        )
        .unwrap();

        let target = Target::from_target_triple(None).unwrap();
        let paths = super::extract_rustflags_library_paths(&manifest_path, &target);

        if let Some(paths) = paths {
            assert_eq!(paths.len(), 2);
            assert!(
                paths
                    .iter()
                    .any(|p| p.to_string_lossy() == "/usr/local/lib")
            );
            assert!(
                paths
                    .iter()
                    .any(|p| p.to_string_lossy() == "/some/other/path")
            );
        } else {
            println!("No rustflags library paths found, which is acceptable");
        }
    }

    #[test]
    fn test_extract_rustflags_library_paths_no_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest_path = temp_dir.path().join("Cargo.toml");

        fs_err::write(
            &manifest_path,
            r#"
[package]
name = "test-package"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();

        let target = Target::from_target_triple(None).unwrap();
        let paths = super::extract_rustflags_library_paths(&manifest_path, &target);

        assert!(paths.is_none());
    }
}
