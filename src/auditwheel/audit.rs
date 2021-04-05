use super::policy::{Policy, POLICIES};
use crate::auditwheel::Manylinux;
use crate::Target;
use anyhow::Result;
use fs_err::File;
use goblin::elf::{sym::STT_FUNC, Elf};
use goblin::strtab::Strtab;
use regex::Regex;
use scroll::Pread;
use std::collections::{HashMap, HashSet};
use std::io;
use std::io::Read;
use std::path::Path;
use thiserror::Error;

/// Error raised during auditing an elf file for manylinux compatibility
#[derive(Error, Debug)]
#[error("Ensuring manylinux compliance failed")]
pub enum AuditWheelError {
    /// The wheel couldn't be read
    #[error("Failed to read the wheel")]
    IoError(#[source] io::Error),
    /// Reexports elfkit parsing errors
    #[error("Goblin failed to parse the elf file")]
    GoblinError(#[source] goblin::error::Error),
    /// The elf file isn't manylinux compatible. Contains the list of offending
    /// libraries.
    #[error(
    "Your library links libpython ({0}), which libraries must not do. Have you forgotten to activate the extension-module feature?",
    )]
    LinksLibPythonError(String),
    /// The elf file isn't manylinux compatible. Contains the list of offending
    /// libraries.
    #[error(
    "Your library is not {0} compliant because it links the following forbidden libraries: {1:?}",
    )]
    ManylinuxValidationError(Policy, Vec<String>),
    /// The elf file isn't manylinux compaible. Contains unsupported architecture
    #[error(
        "Your library is not manylinux compliant because it has unsupported architecture: {0}"
    )]
    UnsupportedArchitecture(String),
}

/// Structure of "version needed" entries is documented in
/// https://refspecs.linuxfoundation.org/LSB_3.0.0/LSB-PDA/LSB-PDA.junk/symversion.html
#[derive(Clone, Copy, Debug, Pread)]
#[repr(C)]
struct GnuVersionNeed {
    /// Version of structure. This value is currently set to 1,
    /// and will be reset if the versioning implementation is incompatibly altered.
    version: u16,
    /// Number of associated verneed array entries.
    cnt: u16,
    /// Offset to the file name string in the section header, in bytes.
    file: u32,
    /// Offset to a corresponding entry in the vernaux array, in bytes.
    aux: u32,
    /// Offset to the next verneed entry, in bytes.
    next: u32,
}

/// Version Needed Auxiliary Entries
#[derive(Clone, Copy, Debug, Pread)]
#[repr(C)]
struct GnuVersionNeedAux {
    /// Dependency name hash value (ELF hash function).
    hash: u32,
    /// Dependency information flag bitmask.
    flags: u16,
    /// Object file version identifier used in the .gnu.version symbol version array.
    /// Bit number 15 controls whether or not the object is hidden; if this bit is set,
    /// the object cannot be used and the static linker will ignore the symbol's presence in the object.
    other: u16,
    /// Offset to the dependency name string in the section header, in bytes.
    name: u32,
    /// Offset to the next vernaux entry, in bytes.
    next: u32,
}

#[derive(Clone, Debug)]
struct VersionedLibrary {
    /// library name
    name: String,
    /// versions needed
    versions: HashSet<String>,
}

/// Find required dynamic linked libraries with version information
fn find_versioned_libraries(
    elf: &Elf,
    buffer: &[u8],
) -> Result<Vec<VersionedLibrary>, AuditWheelError> {
    let mut symbols = Vec::new();
    let section = elf
        .section_headers
        .iter()
        .find(|h| &elf.shdr_strtab[h.sh_name] == ".gnu.version_r");
    if let Some(section) = section {
        let linked_section = &elf.section_headers[section.sh_link as usize];
        linked_section
            .check_size(buffer.len())
            .map_err(AuditWheelError::GoblinError)?;
        let strtab = Strtab::parse(
            buffer,
            linked_section.sh_offset as usize,
            linked_section.sh_size as usize,
            0x0,
        )
        .map_err(AuditWheelError::GoblinError)?;
        let num_versions = section.sh_info as usize;
        let mut offset = section.sh_offset as usize;
        for _ in 0..num_versions {
            let ver = buffer
                .gread::<GnuVersionNeed>(&mut offset)
                .map_err(goblin::error::Error::Scroll)
                .map_err(AuditWheelError::GoblinError)?;
            let mut versions = HashSet::new();
            for _ in 0..ver.cnt {
                let ver_aux = buffer
                    .gread::<GnuVersionNeedAux>(&mut offset)
                    .map_err(goblin::error::Error::Scroll)
                    .map_err(AuditWheelError::GoblinError)?;
                let aux_name = &strtab[ver_aux.name as usize];
                versions.insert(aux_name.to_string());
            }
            let name = &strtab[ver.file as usize];
            // Skip dynamic linker/loader
            if name.starts_with("ld-linux") || name == "ld64.so.2" || name == "ld64.so.1" {
                continue;
            }
            symbols.push(VersionedLibrary {
                name: name.to_string(),
                versions,
            });
        }
    }
    Ok(symbols)
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
            let name = strtab
                .get(sym.st_name)
                .unwrap_or(Ok("BAD NAME"))
                .map_err(AuditWheelError::GoblinError)?;
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
    let arch_versions = &policy
        .symbol_versions
        .get(arch)
        .ok_or_else(|| AuditWheelError::UnsupportedArchitecture(arch.to_string()))?;
    let mut offenders = HashSet::new();
    for dep in deps {
        // Skip dynamic linker/loader
        if dep.starts_with("ld-linux") || dep == "ld64.so.2" || dep == "ld64.so.1" {
            continue;
        }
        if !policy.lib_whitelist.contains(dep) {
            offenders.insert(dep.clone());
        }
    }
    for library in versioned_libraries {
        if !policy.lib_whitelist.contains(&library.name) {
            offenders.insert(library.name.clone());
            continue;
        }
        let mut versions: HashMap<String, HashSet<String>> = HashMap::new();
        for v in &library.versions {
            let mut parts = v.splitn(2, '_');
            let name = parts.next().unwrap();
            let version = parts.next().unwrap();
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
                let offending_symbols = find_incompliant_symbols(&elf, &offending_symbol_versions)?;
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
                offenders.insert(offender);
            }
        }
    }
    // Checks if we can give a more helpful error message
    let is_libpython = Regex::new(r"^libpython3\.\d+\.so\.\d+\.\d+$").unwrap();
    let offenders: Vec<String> = offenders.into_iter().collect();
    match offenders.as_slice() {
        [] => Ok(()),
        [lib] if is_libpython.is_match(lib) => {
            Err(AuditWheelError::LinksLibPythonError(lib.clone()))
        }
        offenders => Err(AuditWheelError::ManylinuxValidationError(
            policy.clone(),
            offenders.to_vec(),
        )),
    }
}

/// An reimplementation of auditwheel, which checks elf files for
/// manylinux compliance.
///
/// If `manylinux`, is None, it returns the the highest matching manylinux policy, or `linux`
/// if nothing else matches. It will error for bogus cases, e.g. if libpython is linked.
///
/// If a specific manylinux version is given, compliance is checked and a warning printed if
/// a higher version would be possible.
///
/// Does nothing for manylinux set to off or non-linux platforms.  
pub fn auditwheel_rs(
    path: &Path,
    target: &Target,
    manylinux: Option<Manylinux>,
) -> Result<Policy, AuditWheelError> {
    if !target.is_linux() || manylinux == Some(Manylinux::Off) {
        return Ok(Policy::default());
    }
    let arch = target.target_arch().to_string();
    let mut file = File::open(path).map_err(AuditWheelError::IoError)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(AuditWheelError::IoError)?;
    let elf = Elf::parse(&buffer).map_err(AuditWheelError::GoblinError)?;
    // This returns essentially the same as ldd
    let deps: Vec<String> = elf.libraries.iter().map(ToString::to_string).collect();
    let versioned_libraries = find_versioned_libraries(&elf, &buffer)?;

    // Find the highest possible manylinux policy, if any
    let mut highest_policy = None;
    for policy in POLICIES.iter() {
        let result = policy_is_satisfied(&policy, &elf, &arch, &deps, &versioned_libraries);
        match result {
            Ok(_) => {
                highest_policy = Some(policy.clone());
                break;
            }
            // UnsupportedArchitecture happens when trying 2010 with aarch64
            Err(AuditWheelError::ManylinuxValidationError(_, _))
            | Err(AuditWheelError::UnsupportedArchitecture(_)) => continue,
            // If there was an error parsing the symbols or libpython was linked,
            // we error no matter what the requested policy was
            Err(err) => return Err(err),
        }
    }

    if let Some(manylinux) = manylinux {
        let policy = POLICIES
            .iter()
            .find(|p| p.name == manylinux.to_string())
            .unwrap();

        if let Some(highest_policy) = highest_policy {
            if policy.priority < highest_policy.priority {
                println!(
                    "📦 Wheel is eligible for a higher priority tag. \
                    You requested {} but this wheel is eligible for {}",
                    policy.name, highest_policy.name,
                );
            }
        }

        match policy_is_satisfied(policy, &elf, &arch, &deps, &versioned_libraries) {
            Ok(_) => Ok(policy.clone()),
            Err(err) => Err(err),
        }
    } else if let Some(policy) = highest_policy {
        Ok(policy)
    } else {
        println!(
            "⚠  Warning: No compatible manylinux tag found, using the linux tag instead. \
            You won't be able to upload those wheels to pypi."
        );

        // Fallback to linux
        Ok(Policy::default())
    }
}
