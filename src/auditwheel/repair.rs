use super::audit::AuditWheelError;
use crate::auditwheel::Policy;
use anyhow::Result;
use fs_err as fs;
use lddtree::DependencyAnalyzer;
use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;

pub fn get_external_libs(
    artifact: impl AsRef<Path>,
    policy: &Policy,
) -> Result<Vec<lddtree::Library>, AuditWheelError> {
    let dep_analyzer = DependencyAnalyzer::new();
    let deps = dep_analyzer.analyze(artifact).unwrap();
    let mut ext_libs = Vec::new();
    for (name, lib) in deps.libraries {
        // Skip dynamic linker/loader and white-listed libs
        if name.starts_with("ld-linux")
            || name == "ld64.so.2"
            || name == "ld64.so.1"
            // musl libc, eg: libc.musl-aarch64.so.1
            || name.starts_with("libc.")
            || policy.lib_whitelist.contains(&name)
        {
            continue;
        }
        ext_libs.push(lib);
    }
    Ok(ext_libs)
}

/// Calculate the sha256 of a file
pub fn hash_file(path: impl AsRef<Path>) -> Result<String, AuditWheelError> {
    let mut file = fs::File::open(path.as_ref()).map_err(AuditWheelError::IoError)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(AuditWheelError::IoError)?;
    let hex = format!("{:x}", hasher.finalize());
    Ok(hex)
}
