use super::musllinux::{find_musl_libc, get_musl_version};
use super::policy::{Policy, MANYLINUX_POLICIES, MUSLLINUX_POLICIES};
use crate::auditwheel::{find_external_libs, PlatformTag};
use crate::compile::BuildArtifact;
use crate::target::Target;
use anyhow::{bail, Context, Result};
use fs_err::File;
use goblin::elf::{sym::STT_FUNC, Elf};
use lddtree::Library;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::io;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

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
    "Your library links libpython ({0}), which libraries must not do. Have you forgotten to activate the extension-module feature?",
    )]
    LinksLibPythonError(String),
    /// The elf file isn't manylinux/musllinux compatible. Contains the list of offending
    /// libraries.
    #[error(
    "Your library is not {0} compliant because it links the following forbidden libraries: {1:?}",
    )]
    LinksForbiddenLibrariesError(Policy, Vec<String>),
    /// The elf file isn't manylinux/musllinux compatible. Contains the list of offending
    /// libraries.
    #[error(
    "Your library is not {0} compliant because of the presence of too-recent versioned symbols: {1:?}. Consider building in a manylinux docker container",
    )]
    VersionedSymbolTooNewError(Policy, Vec<String>),
    /// The elf file isn't manylinux/musllinux compatible. Contains the list of offending
    /// libraries with blacked-list symbols.
    #[error("Your library is not {0} compliant because it depends on black-listed symbols: {1:?}")]
    BlackListedSymbolsError(Policy, Vec<String>),
    /// The elf file isn't manylinux/musllinux compaible. Contains unsupported architecture
    #[error("Your library is not {0} compliant because it has unsupported architecture: {1}")]
    UnsupportedArchitecture(Policy, String),
    /// This platform tag isn't defined by auditwheel yet
    #[error("{0} compatibility policy is not defined by auditwheel yet, pass `--skip-auditwheel` to proceed anyway")]
    UndefinedPolicy(String),
    /// Failed to analyze external shared library dependencies of the wheel
    #[error("Failed to analyze external shared library dependencies of the wheel")]
    DependencyAnalysisError(#[source] lddtree::Error),
}

#[derive(Clone, Debug)]
pub struct VersionedLibrary {
    /// library name
    pub name: String,
    /// versions needed
    versions: HashSet<String>,
}

/// Find required dynamic linked libraries with version information
pub fn find_versioned_libraries(elf: &Elf) -> Vec<VersionedLibrary> {
    let mut symbols = Vec::new();
    if let Some(verneed) = &elf.verneed {
        for need_file in verneed.iter() {
            if let Some(name) = elf.dynstrtab.get_at(need_file.vn_file) {
                // Skip dynamic linker/loader
                if name.starts_with("ld-linux") || name == "ld64.so.2" || name == "ld64.so.1" {
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
                if name.ends_with(&format!("@{}", symbol_version)) {
                    symbols.push(name.to_string());
                }
            }
        }
    }
    Ok(symbols)
}

fn policy_is_satisfied(
    policy: &Policy,
    elf: &Elf,
    arch: &str,
    deps: &[String],
    versioned_libraries: &[VersionedLibrary],
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
            if sym.st_shndx == goblin::elf::section_header::SHN_UNDEF as usize {
                elf.dynstrtab.get_at(sym.st_name).map(ToString::to_string)
            } else {
                None
            }
        })
        .collect();
    for dep in deps {
        // Skip dynamic linker/loader
        if dep.starts_with("ld-linux") || dep == "ld64.so.2" || dep == "ld64.so.1" {
            continue;
        }
        if !policy.lib_whitelist.contains(dep) {
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
        let mut versions: HashMap<String, HashSet<String>> = HashMap::new();
        for v in &library.versions {
            let (name, version) = v.split_once('_').unwrap();
            versions
                .entry(name.to_string())
                .or_default()
                .insert(version.to_string());
        }
        for (name, versions_needed) in versions.iter() {
            let versions_allowed = &arch_versions[name];
            if !versions_needed.is_subset(versions_allowed) {
                let offending_versions: Vec<&str> = versions_needed
                    .difference(versions_allowed)
                    .map(|v| v.as_ref())
                    .collect();
                let offending_symbol_versions: Vec<String> = offending_versions
                    .iter()
                    .map(|v| format!("{}_{}", name, v))
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
    let is_libpython = Regex::new(r"^libpython3\.\d+\.so\.\d+\.\d+$").unwrap();
    let offenders: Vec<String> = offending_libs.into_iter().collect();
    match offenders.as_slice() {
        [] => Ok(()),
        [lib] if is_libpython.is_match(lib) => {
            Err(AuditWheelError::LinksLibPythonError(lib.clone()))
        }
        offenders => Err(AuditWheelError::LinksForbiddenLibrariesError(
            policy.clone(),
            offenders.to_vec(),
        )),
    }
}

fn get_default_platform_policies() -> Vec<Policy> {
    if let Ok(Some(musl_libc)) = find_musl_libc() {
        if let Ok(Some((major, minor))) = get_musl_version(&musl_libc) {
            return MUSLLINUX_POLICIES
                .iter()
                .filter(|policy| {
                    policy.name == "linux"
                        || policy.name == format!("musllinux_{}_{}", major, minor)
                })
                .cloned()
                .collect();
        }
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
pub fn auditwheel_rs(
    artifact: &BuildArtifact,
    target: &Target,
    platform_tag: Option<PlatformTag>,
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
        Some(PlatformTag::Musllinux { x, y }) => MUSLLINUX_POLICIES
            .clone()
            .into_iter()
            .filter(|policy| {
                policy.name == "linux" || policy.name == format!("musllinux_{}_{}", x, y)
            })
            .map(|mut policy| {
                policy.fixup_musl_libc_so_name(target.target_arch());
                policy
            })
            .collect(),
        None => {
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
        let result = policy_is_satisfied(policy, &elf, &arch, &deps, &versioned_libraries);
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
        let tag = platform_tag.to_string();
        let mut policy = Policy::from_name(&tag).ok_or(AuditWheelError::UndefinedPolicy(tag))?;
        policy.fixup_musl_libc_so_name(target.target_arch());

        if let Some(highest_policy) = highest_policy {
            // Don't recommend manylinux1 because rust doesn't support it anymore
            if policy.priority < highest_policy.priority && highest_policy.name != "manylinux_2_5" {
                println!(
                    "📦 Wheel is eligible for a higher priority tag. \
                    You requested {} but this wheel is eligible for {}",
                    policy, highest_policy,
                );
            }
        }

        match policy_is_satisfied(&policy, &elf, &arch, &deps, &versioned_libraries) {
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
    } else {
        println!(
            "⚠️  Warning: No compatible platform tag found, using the linux tag instead. \
            You won't be able to upload those wheels to PyPI."
        );

        // Fallback to linux
        Ok(Policy::default())
    }?;
    Ok((policy, should_repair))
}

/// Get sysroot path from target C compiler
///
/// Currently only gcc is supported, clang doesn't have a `--print-sysroot` option
pub fn get_sysroot_path(target: &Target) -> Result<PathBuf> {
    use std::process::{Command, Stdio};

    if let Some(sysroot) = std::env::var_os("TARGET_SYSROOT") {
        return Ok(PathBuf::from(sysroot));
    }

    let host_triple = target.host_triple();
    let target_triple = target.target_triple();
    if host_triple != target_triple {
        let mut build = cc::Build::new();
        build
            // Suppress cargo metadata for example env vars printing
            .cargo_metadata(false)
            // opt_level, host and target are required
            .opt_level(0)
            .host(host_triple)
            .target(target_triple);
        let compiler = build
            .try_get_compiler()
            .with_context(|| format!("Failed to get compiler for {}", target_triple))?;
        // Only GNU like compilers support `--print-sysroot`
        if !compiler.is_like_gnu() {
            return Ok(PathBuf::from("/"));
        }
        let path = compiler.path();
        let out = Command::new(path)
            .arg("--print-sysroot")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .with_context(|| format!("Failed to run `{} --print-sysroot`", path.display()))?;
        if out.status.success() {
            let sysroot = String::from_utf8(out.stdout)
                .context("Failed to read the sysroot path")?
                .trim()
                .to_owned();
            return Ok(PathBuf::from(sysroot));
        } else {
            bail!(
                "Failed to get the sysroot path: {}",
                String::from_utf8(out.stderr)?
            );
        }
    }
    Ok(PathBuf::from("/"))
}

/// For the given compilation result, return the manylinux platform and the external libs
/// we need to add to repair it
pub fn get_policy_and_libs(
    artifact: &BuildArtifact,
    platform_tag: Option<PlatformTag>,
    target: &Target,
) -> Result<(Policy, Vec<Library>)> {
    let (policy, should_repair) =
        auditwheel_rs(artifact, target, platform_tag).with_context(|| {
            if let Some(platform_tag) = platform_tag {
                format!("Error ensuring {} compliance", platform_tag)
            } else {
                "Error checking for manylinux/musllinux compliance".to_string()
            }
        })?;
    let external_libs = if should_repair {
        let sysroot = get_sysroot_path(target).unwrap_or_else(|_| PathBuf::from("/"));
        let ld_paths = artifact.linked_paths.iter().map(PathBuf::from).collect();
        find_external_libs(&artifact.path, &policy, sysroot, ld_paths).with_context(|| {
            if let Some(platform_tag) = platform_tag {
                format!("Error repairing wheel for {} compliance", platform_tag)
            } else {
                "Error repairing wheel for manylinux/musllinux compliance".to_string()
            }
        })?
    } else {
        Vec::new()
    };
    Ok((policy, external_libs))
}

pub fn relpath(to: &Path, from: &Path) -> PathBuf {
    let mut suffix_pos = 0;
    for (f, t) in from.components().zip(to.components()) {
        if f == t {
            suffix_pos += 1;
        } else {
            break;
        }
    }
    let mut result = PathBuf::new();
    from.components()
        .skip(suffix_pos)
        .map(|_| result.push(".."))
        .last();
    to.components()
        .skip(suffix_pos)
        .map(|x| result.push(x.as_os_str()))
        .last();
    result
}

#[cfg(test)]
mod test {
    use crate::auditwheel::audit::relpath;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    #[test]
    fn test_relpath() {
        let cases = [
            ("", "", ""),
            ("/", "/usr", ".."),
            ("/", "/usr/lib", "../.."),
        ];
        for (from, to, expected) in cases {
            let from = Path::new(from);
            let to = Path::new(to);
            let result = relpath(from, to);
            assert_eq!(result, Path::new(expected));
        }
    }
}
