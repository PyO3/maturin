use super::musllinux::{find_musl_libc, get_musl_version};
use super::policy::{MANYLINUX_POLICIES, MUSLLINUX_POLICIES, Policy};
use crate::auditwheel::PlatformTag;
use crate::compile::BuildArtifact;
use crate::target::{Arch, Target};
use anyhow::{Context, Result, bail};
use fs_err::File;
use goblin::elf::{Elf, sym::STB_WEAK, sym::STT_FUNC};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::{fmt, io};
use thiserror::Error;

pub(crate) static IS_LIBPYTHON: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^libpython3\.\d+m?u?t?\.so\.\d+\.\d+$").unwrap());

/// Returns `true` if the given shared-library name is a dynamic linker
/// (e.g. `ld-linux-x86-64.so.2`, `ld64.so.2`, `ld-musl-*.so.1`).
pub(crate) fn is_dynamic_linker(name: &str) -> bool {
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

/// Auditwheel mode
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum AuditWheelMode {
    /// Audit and repair wheel for manylinux compliance
    #[default]
    Repair,
    /// Check wheel for manylinux compliance, but do not repair
    Check,
    /// Don't check for manylinux compliance
    Skip,
}

impl fmt::Display for AuditWheelMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuditWheelMode::Repair => write!(f, "repair"),
            AuditWheelMode::Check => write!(f, "check"),
            AuditWheelMode::Skip => write!(f, "skip"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct VersionedLibrary {
    /// library name
    pub name: String,
    /// versions needed
    versions: HashSet<String>,
}

impl VersionedLibrary {
    /// Parse version strings (e.g. "GLIBC_2.17") into a map of name -> set of versions.
    /// e.g. {"GLIBC" -> {"2.17", "2.5"}, "GCC" -> {"3.0"}}
    ///
    pub(crate) fn parsed_versions(&self) -> HashMap<String, HashSet<String>> {
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
pub fn find_versioned_libraries(elf: &Elf) -> Vec<VersionedLibrary> {
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

pub(crate) fn get_default_platform_policies() -> Vec<Policy> {
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
pub fn auditwheel_rs(
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
            .with_context(|| format!("Failed to get compiler for {target_triple}"))?;
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
            if sysroot.is_empty() {
                return Ok(PathBuf::from("/"));
            } else {
                return Ok(PathBuf::from(sysroot));
            }
        } else {
            bail!(
                "Failed to get the sysroot path: {}",
                String::from_utf8(out.stderr)?
            );
        }
    }
    Ok(PathBuf::from("/"))
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
mod tests {
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
