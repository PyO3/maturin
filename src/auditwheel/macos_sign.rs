//! Pure-Rust ad-hoc code signing for Mach-O binaries.
//!
//! This module provides ad-hoc code signing for both thin and fat (universal) Mach-O
//! binaries. On macOS, it uses Apple's `codesign` CLI tool directly. For cross-compilation
//! from other platforms, it uses `arwen-codesign` as a pure-Rust fallback.
//!
//! The signatures produced are functionally equivalent to those created by Apple's
//! `codesign -s -` command and pass all verification checks.

use anyhow::{Context, Result};
#[cfg(not(target_os = "macos"))]
use arwen_codesign::{AdhocSignOptions, adhoc_sign};
#[cfg(not(target_os = "macos"))]
use fat_macho::{Error as FatMachoError, FatReader, FatWriter};
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(not(target_os = "macos"))]
use tempfile::NamedTempFile;

/// Ad-hoc codesign Mach-O bytes, handling both thin and fat (universal) binaries.
///
/// For fat binaries, each architecture slice is signed individually and then
/// the slices are reassembled into a new fat binary. This approach requires that
/// each thin slice has an existing `LC_CODE_SIGNATURE` load command (which is the
/// case for binaries produced by modern Apple toolchains with `-Wl,-adhoc_codesign`).
#[cfg(not(target_os = "macos"))]
fn ad_hoc_sign_macho_bytes(data: Vec<u8>, identifier: &str) -> Result<Vec<u8>> {
    match FatReader::new(&data) {
        Ok(reader) => {
            let mut writer = FatWriter::new();
            for arch in reader.iter_arches() {
                let arch = arch.with_context(|| {
                    format!("Failed to iterate fat Mach-O slices for {identifier}")
                })?;
                let signed = sign_thin_macho_slice(arch.slice(&data).to_vec(), identifier)?;
                writer.add(signed).with_context(|| {
                    format!("Failed to rebuild fat Mach-O slices for {identifier}")
                })?;
            }

            let mut rebuilt = Vec::new();
            writer
                .write_to(&mut rebuilt)
                .with_context(|| format!("Failed to write fat Mach-O for {identifier}"))?;
            Ok(rebuilt)
        }
        Err(FatMachoError::NotFatBinary) => sign_thin_macho_slice(data, identifier),
        Err(err) => {
            Err(err).with_context(|| format!("Failed to parse fat Mach-O for {identifier}"))
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn sign_thin_macho_slice(data: Vec<u8>, identifier: &str) -> Result<Vec<u8>> {
    adhoc_sign(data, &AdhocSignOptions::new(identifier))
        .with_context(|| format!("Failed to ad-hoc codesign Mach-O slice {identifier}"))
}

/// Ad-hoc codesign a Mach-O file at the given path.
///
/// On macOS, uses Apple's `codesign` CLI tool directly. For cross-compilation
/// from other platforms, uses the pure-Rust `arwen-codesign` library.
#[cfg(target_os = "macos")]
pub(crate) fn ad_hoc_sign(path: &Path) -> Result<()> {
    let output = Command::new("codesign")
        .args(["-s", "-", "-f"])
        .arg(path)
        .output()
        .context("Failed to run codesign command")?;

    if !output.status.success() {
        anyhow::bail!(
            "codesign failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Ad-hoc codesign a Mach-O file at the given path.
///
/// Uses the pure-Rust `arwen-codesign` library for cross-compilation scenarios
/// where Apple's `codesign` tool is not available.
#[cfg(not(target_os = "macos"))]
pub(crate) fn ad_hoc_sign(path: &Path) -> Result<()> {
    let data = fs_err::read(path)?;
    let identifier = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");

    let signed = ad_hoc_sign_macho_bytes(data, identifier)?;
    let metadata = fs_err::metadata(path)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = NamedTempFile::new_in(parent)?;
    use std::io::Write;
    temp.write_all(&signed)?;
    temp.as_file().sync_all()?;
    fs_err::set_permissions(temp.path(), metadata.permissions())?;
    temp.persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("Failed to persist signed Mach-O {}", path.display()))?;
    Ok(())
}

#[cfg(target_os = "macos")]
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Check if the given bytes represent a fat (universal) Mach-O binary.
    ///
    /// Fat binaries use magic `0xcafebabe` (big-endian) or `0xbebafeca` (little-endian).
    fn is_fat_macho(data: &[u8]) -> bool {
        matches!(
            data.get(..4),
            Some([0xca, 0xfe, 0xba, 0xbe] | [0xbe, 0xba, 0xfe, 0xca])
        )
    }

    /// Compile a minimal Mach-O binary for the given architecture.
    /// Returns the path to the compiled binary.
    fn compile_thin_macho(dir: &Path, arch: &str) -> std::path::PathBuf {
        /// Minimal C source that compiles to a tiny Mach-O executable.
        const MINIMAL_C_SOURCE: &str = "int main(){return 0;}";

        let src = dir.join("main.c");
        let out = dir.join(format!("main_{arch}"));
        fs_err::write(&src, MINIMAL_C_SOURCE).unwrap();

        let status = Command::new("clang")
            .args([
                "-arch",
                arch,
                // Ensure LC_CODE_SIGNATURE is present even when cross-compiling
                "-Wl,-adhoc_codesign",
                "-o",
            ])
            .arg(&out)
            .arg(&src)
            .status()
            .expect("Failed to run clang");
        assert!(status.success(), "clang failed for {arch}");
        out
    }

    #[test]
    fn signs_thin_binary_and_verifies() {
        let temp_dir = tempfile::tempdir().unwrap();
        let thin = compile_thin_macho(temp_dir.path(), "arm64");

        ad_hoc_sign(&thin).unwrap();

        let output = Command::new("codesign")
            .args(["--verify", "--verbose"])
            .arg(&thin)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "codesign --verify failed for thin binary: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn signs_thin_x86_64_binary_and_verifies() {
        let temp_dir = tempfile::tempdir().unwrap();
        let thin = compile_thin_macho(temp_dir.path(), "x86_64");

        ad_hoc_sign(&thin).unwrap();

        let output = Command::new("codesign")
            .args(["--verify", "--verbose"])
            .arg(&thin)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "codesign --verify failed for thin x86_64 binary: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn signs_fat_binary_from_thin_slices() {
        use fat_macho::{FatReader, FatWriter};

        // Test the fat binary signing flow by building thin binaries,
        // manually creating a fat binary with FatWriter, and signing it.
        // This simulates what happens when bundling dylibs from different
        // architectures into a universal binary.
        let temp_dir = tempfile::tempdir().unwrap();
        let arm64 = compile_thin_macho(temp_dir.path(), "arm64");
        let x86_64 = compile_thin_macho(temp_dir.path(), "x86_64");

        // Read the thin binaries (each has its own LC_CODE_SIGNATURE)
        let arm64_data = fs_err::read(&arm64).unwrap();
        let x86_64_data = fs_err::read(&x86_64).unwrap();

        // Build fat binary from self-contained thin slices
        let mut writer = FatWriter::new();
        writer.add(arm64_data).unwrap();
        writer.add(x86_64_data).unwrap();

        let mut fat = Vec::new();
        writer.write_to(&mut fat).unwrap();

        // Verify it's a fat binary
        assert!(is_fat_macho(&fat), "Expected fat binary");

        // Write to file and sign with codesign CLI
        let fat_path = temp_dir.path().join("universal");
        fs_err::write(&fat_path, &fat).unwrap();

        ad_hoc_sign(&fat_path).unwrap();

        // Verify both slices are present after signing
        let signed = fs_err::read(&fat_path).unwrap();
        let reader = FatReader::new(&signed).unwrap();
        assert!(reader.extract("arm64").is_some(), "arm64 slice missing");
        assert!(reader.extract("x86_64").is_some(), "x86_64 slice missing");

        // Verify with codesign
        let output = Command::new("codesign")
            .args(["--verify", "--verbose"])
            .arg(&fat_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "codesign --verify failed for fat binary: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
