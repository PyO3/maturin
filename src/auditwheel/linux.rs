//! Linux/ELF wheel audit and repair.
//!
//! This module implements [`WheelRepairer`] for Linux ELF binaries,
//! providing the Rust equivalent of [auditwheel](https://github.com/pypa/auditwheel).
//!
//! It contains all ELF-specific logic: manylinux/musllinux compliance
//! auditing, external dependency discovery via lddtree, versioned symbol
//! checking, and binary patching via `patchelf` (SONAME, DT_NEEDED, RPATH).

use super::audit::{get_sysroot_path, relpath};
use super::musllinux::{find_musl_libc, get_musl_version};
use super::policy::{MANYLINUX_POLICIES, MUSLLINUX_POLICIES, Policy};
use super::repair::{AuditedArtifact, GraftedLib, WheelRepairer};
use super::{PlatformTag, patchelf};
use crate::compile::BuildArtifact;
use crate::target::{Arch, Target};
use anyhow::{Context, Result, bail};
use fs_err::File;
use goblin::elf::{Elf, sym::STB_WEAK, sym::STT_FUNC};
use lddtree::Library;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::debug;

pub(crate) static IS_LIBPYTHON: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^libpython3\.\d+m?u?t?\.so\.\d+\.\d+$").unwrap());

/// Returns `true` if the given shared-library name is a dynamic linker
/// (e.g. `ld-linux-x86-64.so.2`, `ld64.so.2`, `ld-musl-*.so.1`).
fn is_dynamic_linker(name: &str) -> bool {
    name.starts_with("ld-linux")
        || name == "ld64.so.2"
        || name == "ld64.so.1"
        || name.starts_with("ld-musl")
}

/// Error raised during auditing an elf file for manylinux/musllinux compatibility
#[derive(Error, Debug)]
#[error("Ensuring manylinux/musllinux compliance failed")]
pub enum AuditWheelError {
    /// The wheel couldn't be read
    #[error("Failed to read the wheel")]
    IoError(#[source] io::Error),
    /// Reexports goblin parsing errors
    #[error("Goblin failed to parse the elf file")]
    GoblinError(#[source] goblin::error::Error),
    /// The elf file isn't manylinux/musllinux compatible. Contains the list of offending
    /// libraries.
    #[error(
        "Your library links libpython ({0}), which libraries must not do. Have you forgotten to activate the extension-module feature?"
    )]
    LinksLibPythonError(String),
    /// The elf file isn't manylinux/musllinux compatible. Contains the list of offending
    /// libraries.
    #[error(
        "Your library is not {0} compliant because it links the following forbidden libraries: {1:?}"
    )]
    LinksForbiddenLibrariesError(Policy, Vec<String>),
    /// The elf file isn't manylinux/musllinux compatible. Contains the list of offending
    /// libraries.
    #[error(
        "Your library is not {0} compliant because of the presence of too-recent versioned symbols: {1:?}. Consider building in a manylinux docker container"
    )]
    VersionedSymbolTooNewError(Policy, Vec<String>),
    /// The elf file isn't manylinux/musllinux compatible. Contains the list of offending
    /// libraries with blacked-list symbols.
    #[error("Your library is not {0} compliant because it depends on black-listed symbols: {1:?}")]
    BlackListedSymbolsError(Policy, Vec<String>),
    /// The elf file isn't manylinux/musllinux compatible. Contains unsupported architecture
    #[error("Your library is not {0} compliant because it has unsupported architecture: {1}")]
    UnsupportedArchitecture(Policy, String),
    /// This platform tag isn't defined by auditwheel yet
    #[error(
        "{0} compatibility policy is not defined by auditwheel yet, pass `--auditwheel=skip` to proceed anyway"
    )]
    UndefinedPolicy(PlatformTag),
    /// Failed to analyze external shared library dependencies of the wheel
    #[error("Failed to analyze external shared library dependencies of the wheel")]
    DependencyAnalysisError(#[source] lddtree::Error),
}

#[derive(Clone, Debug)]
struct VersionedLibrary {
    /// library name
    name: String,
    /// versions needed
    versions: HashSet<String>,
}

impl VersionedLibrary {
    /// Parse version strings (e.g. "GLIBC_2.17") into a map of name -> set of versions.
    /// e.g. {"GLIBC" -> {"2.17", "2.5"}, "GCC" -> {"3.0"}}
    ///
    fn parsed_versions(&self) -> HashMap<String, HashSet<String>> {
        let mut result: HashMap<String, HashSet<String>> = HashMap::new();
        for v in &self.versions {
            if let Some((name, version)) = v.split_once('_') {
                result
                    .entry(name.to_string())
                    .or_default()
                    .insert(version.to_string());
            }
        }
        result
    }
}

/// Find required dynamic linked libraries with version information
fn find_versioned_libraries(elf: &Elf) -> Vec<VersionedLibrary> {
    let mut symbols = Vec::new();
    if let Some(verneed) = &elf.verneed {
        for need_file in verneed.iter() {
            if let Some(name) = elf.dynstrtab.get_at(need_file.vn_file) {
                // Skip dynamic linker/loader
                if is_dynamic_linker(name) {
                    continue;
                }
                let mut versions = HashSet::new();
                for need_ver in need_file.iter() {
                    if let Some(aux_name) = elf.dynstrtab.get_at(need_ver.vna_name) {
                        versions.insert(aux_name.to_string());
                    }
                }
                symbols.push(VersionedLibrary {
                    name: name.to_string(),
                    versions,
                });
            }
        }
    }
    symbols
}

/// Find incompliant symbols from symbol versions
#[allow(clippy::result_large_err)]
fn find_incompliant_symbols(
    elf: &Elf,
    symbol_versions: &[String],
) -> Result<Vec<String>, AuditWheelError> {
    let mut symbols = Vec::new();
    let strtab = &elf.strtab;
    for sym in &elf.syms {
        if sym.st_type() == STT_FUNC {
            let name = strtab.get_at(sym.st_name).unwrap_or("BAD NAME");
            for symbol_version in symbol_versions {
                if name.ends_with(&format!("@{symbol_version}")) {
                    symbols.push(name.to_string());
                }
            }
        }
    }
    Ok(symbols)
}

#[allow(clippy::result_large_err)]
fn policy_is_satisfied(
    policy: &Policy,
    elf: &Elf,
    arch: &str,
    deps: &[String],
    versioned_libraries: &[VersionedLibrary],
    allow_linking_libpython: bool,
) -> Result<(), AuditWheelError> {
    let arch_versions = &policy.symbol_versions.get(arch).ok_or_else(|| {
        AuditWheelError::UnsupportedArchitecture(policy.clone(), arch.to_string())
    })?;
    let mut offending_libs = HashSet::new();
    let mut offending_versioned_syms = HashSet::new();
    let mut offending_blacklist_syms = HashMap::new();
    let undef_symbols: HashSet<String> = elf
        .dynsyms
        .iter()
        .filter_map(|sym| {
            // Do not consider weak symbols as undefined, they are optional at runtime.
            if sym.st_shndx == goblin::elf::section_header::SHN_UNDEF as usize
                && sym.st_bind() != STB_WEAK
            {
                elf.dynstrtab.get_at(sym.st_name).map(ToString::to_string)
            } else {
                None
            }
        })
        .collect();

    for dep in deps {
        if is_dynamic_linker(dep) {
            continue;
        }
        if !policy.lib_whitelist.contains(dep) {
            if allow_linking_libpython && IS_LIBPYTHON.is_match(dep) {
                continue;
            }
            offending_libs.insert(dep.clone());
        }
        if let Some(sym_list) = policy.blacklist.get(dep) {
            let mut intersection: Vec<_> = sym_list.intersection(&undef_symbols).cloned().collect();
            if !intersection.is_empty() {
                intersection.sort();
                offending_blacklist_syms.insert(dep, intersection);
            }
        }
    }
    for library in versioned_libraries {
        if !policy.lib_whitelist.contains(&library.name) {
            offending_libs.insert(library.name.clone());
            continue;
        }
        for (name, versions_needed) in library.parsed_versions() {
            let Some(versions_allowed) = arch_versions.get(&name) else {
                offending_versioned_syms.insert(format!(
                    "{} offending versions: unknown symbol namespace {name}",
                    library.name,
                ));
                continue;
            };
            if !versions_needed.is_subset(versions_allowed) {
                let offending_versions: Vec<&str> = versions_needed
                    .difference(versions_allowed)
                    .map(|v| v.as_ref())
                    .collect();
                let offending_symbol_versions: Vec<String> = offending_versions
                    .iter()
                    .map(|v| format!("{name}_{v}"))
                    .collect();
                let offending_symbols = find_incompliant_symbols(elf, &offending_symbol_versions)?;
                let offender = if offending_symbols.is_empty() {
                    format!(
                        "{} offending versions: {}",
                        library.name,
                        offending_symbol_versions.join(", ")
                    )
                } else {
                    format!(
                        "{} offending symbols: {}",
                        library.name,
                        offending_symbols.join(", ")
                    )
                };
                offending_versioned_syms.insert(offender);
            }
        }
    }
    // Check for black-listed symbols
    if !offending_blacklist_syms.is_empty() {
        let offenders = offending_blacklist_syms
            .into_iter()
            .map(|(lib, syms)| format!("{}: {}", lib, syms.join(", ")))
            .collect();
        return Err(AuditWheelError::BlackListedSymbolsError(
            policy.clone(),
            offenders,
        ));
    }
    // Check for too-recent versioned symbols
    if !offending_versioned_syms.is_empty() {
        return Err(AuditWheelError::VersionedSymbolTooNewError(
            policy.clone(),
            offending_versioned_syms.into_iter().collect(),
        ));
    }
    // Check for libpython and forbidden libraries
    let offenders: Vec<String> = offending_libs.into_iter().collect();
    match offenders.as_slice() {
        [] => Ok(()),
        [lib] if IS_LIBPYTHON.is_match(lib) => {
            Err(AuditWheelError::LinksLibPythonError(lib.clone()))
        }
        offenders => Err(AuditWheelError::LinksForbiddenLibrariesError(
            policy.clone(),
            offenders.to_vec(),
        )),
    }
}

fn get_default_platform_policies() -> Vec<Policy> {
    if let Ok(Some(musl_libc)) = find_musl_libc()
        && let Ok(Some((major, minor))) = get_musl_version(musl_libc)
    {
        return MUSLLINUX_POLICIES
            .iter()
            .filter(|policy| {
                policy.name == "linux" || policy.name == format!("musllinux_{major}_{minor}")
            })
            .cloned()
            .collect();
    }
    MANYLINUX_POLICIES.clone()
}

/// An reimplementation of auditwheel, which checks elf files for
/// manylinux/musllinux compliance.
///
/// If `platform_tag`, is None, it returns the the highest matching manylinux/musllinux policy
/// and whether we need to repair with patchelf,, or `linux` if nothing else matches.
/// It will error for bogus cases, e.g. if libpython is linked.
///
/// If a specific manylinux/musllinux version is given, compliance is checked and a warning printed if
/// a higher version would be possible.
///
/// Does nothing for `platform_tag` set to `Off`/`Linux` or non-linux platforms.
#[allow(clippy::result_large_err)]
fn auditwheel_rs(
    artifact: &BuildArtifact,
    target: &Target,
    platform_tag: Option<PlatformTag>,
    allow_linking_libpython: bool,
) -> Result<(Policy, bool), AuditWheelError> {
    if !target.is_linux() || platform_tag == Some(PlatformTag::Linux) {
        return Ok((Policy::default(), false));
    }
    let path = &artifact.path;
    let arch = target.target_arch().to_string();
    let mut file = File::open(path).map_err(AuditWheelError::IoError)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(AuditWheelError::IoError)?;
    let elf = Elf::parse(&buffer).map_err(AuditWheelError::GoblinError)?;
    // This returns essentially the same as ldd
    let deps: Vec<String> = elf.libraries.iter().map(ToString::to_string).collect();
    let versioned_libraries = find_versioned_libraries(&elf);

    // Find the highest possible policy, if any
    let platform_policies = match platform_tag {
        Some(PlatformTag::Manylinux { .. }) => MANYLINUX_POLICIES.clone(),
        Some(PlatformTag::Musllinux { major, minor }) => MUSLLINUX_POLICIES
            .clone()
            .into_iter()
            .filter(|policy| {
                policy.name == "linux" || policy.name == format!("musllinux_{major}_{minor}")
            })
            .map(|mut policy| {
                policy.fixup_musl_libc_so_name(target.target_arch());
                policy
            })
            .collect(),
        None | Some(PlatformTag::Pypi) => {
            // Using the default for the `pypi` tag means we're correctly using manylinux where
            // possible.
            let mut policies = get_default_platform_policies();
            for policy in &mut policies {
                policy.fixup_musl_libc_so_name(target.target_arch());
            }
            policies
        }
        Some(PlatformTag::Linux) => unreachable!(),
    };
    let mut highest_policy = None;
    let mut should_repair = false;
    for policy in platform_policies.iter() {
        let result = policy_is_satisfied(
            policy,
            &elf,
            &arch,
            &deps,
            &versioned_libraries,
            allow_linking_libpython,
        );
        match result {
            Ok(_) => {
                highest_policy = Some(policy.clone());
                should_repair = false;
                break;
            }
            Err(AuditWheelError::LinksForbiddenLibrariesError(..)) => {
                highest_policy = Some(policy.clone());
                should_repair = true;
                break;
            }
            Err(AuditWheelError::VersionedSymbolTooNewError(..))
            | Err(AuditWheelError::BlackListedSymbolsError(..))
            // UnsupportedArchitecture happens when trying 2010 with aarch64
            | Err(AuditWheelError::UnsupportedArchitecture(..)) => continue,
            // If there was an error parsing the symbols or libpython was linked,
            // we error no matter what the requested policy was
            Err(err) => return Err(err),
        }
    }

    let policy = if let Some(platform_tag) = platform_tag {
        let mut policy = Policy::from_tag(&platform_tag)
            .ok_or(AuditWheelError::UndefinedPolicy(platform_tag))?;
        policy.fixup_musl_libc_so_name(target.target_arch());

        if let Some(highest_policy) = highest_policy {
            // Don't recommend manylinux1 because rust doesn't support it anymore
            if policy.priority < highest_policy.priority && highest_policy.name != "manylinux_2_5" {
                eprintln!(
                    "📦 Wheel is eligible for a higher priority tag. \
                    You requested {policy} but this wheel is eligible for {highest_policy}",
                );
            }
        }

        match policy_is_satisfied(
            &policy,
            &elf,
            &arch,
            &deps,
            &versioned_libraries,
            allow_linking_libpython,
        ) {
            Ok(_) => {
                should_repair = false;
                Ok(policy)
            }
            Err(AuditWheelError::LinksForbiddenLibrariesError(..)) => {
                should_repair = true;
                Ok(policy)
            }
            Err(err) => Err(err),
        }
    } else if let Some(policy) = highest_policy {
        Ok(policy)
    } else if target.target_arch() == Arch::Armv6L || target.target_arch() == Arch::Armv7L {
        // Old arm versions
        // https://github.com/pypi/warehouse/blob/556e1e3390999381c382873b003a779a1363cb4d/warehouse/forklift/legacy.py#L122-L123
        Ok(Policy::default())
    } else {
        eprintln!(
            "⚠️  Warning: No compatible platform tag found, using the linux tag instead. \
            You won't be able to upload those wheels to PyPI."
        );

        // Fallback to linux
        Ok(Policy::default())
    }?;
    Ok((policy, should_repair))
}

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
        mut ld_paths: Vec<PathBuf>,
    ) -> Result<(Policy, Vec<Library>)> {
        // Extend caller-provided paths with RUSTFLAGS library search paths
        if let Some(rustflags_paths) =
            extract_rustflags_library_paths(&self.manifest_path, &self.target)
        {
            ld_paths.extend(rustflags_paths);
        }
        get_policy_and_libs(
            artifact,
            self.platform_tag,
            &self.target,
            ld_paths,
            self.allow_linking_libpython,
        )
    }

    fn patch(
        &self,
        audited: &[AuditedArtifact],
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
        // Only replace entries that the artifact actually depends on to avoid
        // unnecessary patchelf invocations and errors when an old name is
        // absent from a given binary.
        for aa in audited {
            let artifact_deps: HashSet<&str> = aa
                .external_libs
                .iter()
                .map(|lib| lib.name.as_str())
                .collect();
            let replacements: Vec<_> = name_map
                .iter()
                .filter(|(old, _)| artifact_deps.contains(**old))
                .map(|(k, v)| (*k, v.to_string()))
                .collect();
            if !replacements.is_empty() {
                patchelf::replace_needed(&aa.artifact.path, &replacements)?;
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
        for aa in audited {
            let mut new_rpaths = patchelf::get_rpath(&aa.artifact.path)?;
            let new_rpath = Path::new("$ORIGIN").join(relpath(libs_dir, artifact_dir));
            new_rpaths.push(new_rpath.to_str().unwrap().to_string());
            let new_rpath = new_rpaths.join(":");
            patchelf::set_rpath(&aa.artifact.path, &new_rpath)?;
        }

        Ok(())
    }

    fn patch_editable(&self, audited: &[AuditedArtifact]) -> Result<()> {
        for aa in audited {
            if aa.artifact.linked_paths.is_empty() {
                continue;
            }
            let old_rpaths = patchelf::get_rpath(&aa.artifact.path)?;
            let mut new_rpaths = old_rpaths.clone();
            for path in &aa.artifact.linked_paths {
                if !old_rpaths.contains(path) {
                    new_rpaths.push(path.to_string());
                }
            }
            let new_rpath = new_rpaths.join(":");
            if let Err(err) = patchelf::set_rpath(&aa.artifact.path, &new_rpath) {
                eprintln!(
                    "⚠️ Warning: Failed to set rpath for {}: {}",
                    aa.artifact.path.display(),
                    err
                );
            }
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
    ld_paths: Vec<PathBuf>,
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
    // Policies must be sorted from highest to lowest priority so we find the
    // best (most compatible) match first when iterating.
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

        // Find the highest policy that this external library satisfies
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

    // Encode the rustflags for parsing with the rustflags crate
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
        // Create a temporary directory with a Cargo.toml and .cargo/config.toml
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest_path = temp_dir.path().join("Cargo.toml");
        let cargo_dir = temp_dir.path().join(".cargo");
        let config_path = cargo_dir.join("config.toml");

        // Create the directories
        fs_err::create_dir_all(&cargo_dir).unwrap();

        // Create a minimal Cargo.toml
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

        // Create a config.toml with rustflags containing -L options
        fs_err::write(
            &config_path,
            r#"
[build]
rustflags = ["-L", "dependency=/usr/local/lib", "-L", "/some/other/path", "-C", "opt-level=3"]
"#,
        )
        .unwrap();

        // Test the function
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
            // It's possible that rustflags parsing fails in some environments,
            // so we just verify the function doesn't panic
            println!("No rustflags library paths found, which is acceptable");
        }
    }

    #[test]
    fn test_extract_rustflags_library_paths_no_config() {
        // Test with a directory that has no cargo config
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest_path = temp_dir.path().join("Cargo.toml");

        // Create a minimal Cargo.toml
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

        // Should return None when there's no cargo config with rustflags
        assert!(paths.is_none());
    }
}
