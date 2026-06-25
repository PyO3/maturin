use crate::target::Target;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

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
    /// Audit wheel and warn about external libraries, but do not fail or repair
    Warn,
    /// Don't check for manylinux compliance
    Skip,
}

impl fmt::Display for AuditWheelMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuditWheelMode::Repair => write!(f, "repair"),
            AuditWheelMode::Check => write!(f, "check"),
            AuditWheelMode::Warn => write!(f, "warn"),
            AuditWheelMode::Skip => write!(f, "skip"),
        }
    }
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

/// Compute the relative path from `from` to `to`.
///
/// Both inputs must be either absolute or relative — mixing them (e.g.
/// one absolute and one relative) would otherwise produce nonsense `..`
/// chains, since their components would never match and the entire `from`
/// tail would be turned into `..` segments. That feeds the RPATH math with
/// garbage, so reject it up front with a clear error.
pub fn relpath(to: &Path, from: &Path) -> Result<PathBuf> {
    let to_is_absolute = to.is_absolute();
    let from_is_absolute = from.is_absolute();
    if to_is_absolute != from_is_absolute {
        bail!(
            "cannot compute relative path between an absolute and a relative path: `{}` and `{}`",
            from.display(),
            to.display()
        );
    }
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
    Ok(result)
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
            let result = relpath(from, to).unwrap();
            assert_eq!(result, Path::new(expected));
        }
    }
}
