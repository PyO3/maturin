//! Windows wheel repair (delvewheel).
//!
//! Finds external (non-system) DLLs that a wheel's native extensions depend on,
//! and copies them into the wheel so the extension finds them at runtime.
//!
//! This is the Rust equivalent of [delvewheel](https://github.com/adang1345/delvewheel).
//! The key operations are:
//! 1. Analyze the extension's dependency tree using lddtree
//! 2. Filter out system DLLs (by name patterns AND by resolved path)
//! 3. Copy external DLLs into `<package>.libs/` inside the wheel
//! 4. Rename DLLs with hash-suffixed names and patch PE import tables
//!    (see [`pe_patch`](super::pe_patch)) to reference the new names
//! 5. Patch `__init__.py` with `os.add_dll_directory()` for runtime DLL
//!    discovery

use super::repair::{AuditResult, AuditedArtifact, GraftedLib, WheelRepairer};
use crate::compile::BuildArtifact;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Windows wheel repairer (delvewheel equivalent).
///
/// Bundles external DLLs and rewrites PE import tables so that
/// hash-suffixed copies are loaded from the `.libs/` directory.
///
/// Like [delvewheel](https://github.com/adang1345/delvewheel), this does not
/// modify the wheel's platform tag — Windows tags (`win_amd64`, `win32`,
/// `win_arm64`) have no version component.
pub struct WindowsRepairer;

impl WheelRepairer for WindowsRepairer {
    fn audit(&self, artifact: &BuildArtifact, ld_paths: Vec<PathBuf>) -> Result<AuditResult> {
        let external_libs = find_external_libs(&artifact.path, ld_paths)
            .context("Failed to find external libraries for Windows wheel")?;
        Ok(AuditResult::new(super::Policy::default(), external_libs))
    }

    fn patch(
        &self,
        audited: &[AuditedArtifact],
        grafted: &[GraftedLib],
        _libs_dir: &Path,
        _artifact_dir: &Path,
    ) -> Result<()> {
        // Build a lookup from original name → new name.
        // Include aliases so all names pointing to the same file are rewritten.
        let mut replacements: Vec<(&str, &str)> = Vec::new();
        for l in grafted {
            if l.original_name != l.new_name {
                replacements.push((l.original_name.as_str(), l.new_name.as_str()));
            }
            for alias in &l.aliases {
                replacements.push((alias.as_str(), l.new_name.as_str()));
            }
        }

        // Patch each artifact's import table to reference the new names
        for aa in audited {
            if !replacements.is_empty() {
                super::pe_patch::replace_needed(&aa.artifact.path, &replacements).with_context(
                    || {
                        format!(
                            "Failed to patch PE imports in {}",
                            aa.artifact.path.display()
                        )
                    },
                )?;
            }
        }

        // Patch cross-references between grafted DLLs and clear DependentLoadFlags
        for lib in grafted {
            let lib_replacements: Vec<(&str, &str)> = lib
                .needed
                .iter()
                .filter_map(|n| {
                    replacements
                        .iter()
                        .find(|(old, _)| old.eq_ignore_ascii_case(n))
                        .map(|(old, new)| (*old, *new))
                })
                .collect();
            if !lib_replacements.is_empty() {
                super::pe_patch::replace_needed(&lib.dest_path, &lib_replacements).with_context(
                    || format!("Failed to patch PE imports in {}", lib.dest_path.display()),
                )?;
            } else {
                super::pe_patch::clear_dependent_load_flags(&lib.dest_path)?;
            }
        }

        Ok(())
    }

    fn init_py_patch(&self, libs_dir_name: &str, depth: usize) -> Option<String> {
        let pardir_chain = "os.pardir, ".repeat(depth);
        Some(format!(
            "# start maturin patch\n\
             def _maturin_dll_patch():\n\
             \x20   import os\n\
             \x20   libs_dir = os.path.abspath(os.path.join(os.path.dirname(__file__), {pardir_chain}\"{libs_dir_name}\"))\n\
             \x20   if os.path.isdir(libs_dir) and hasattr(os, 'add_dll_directory'):\n\
             \x20       os.add_dll_directory(libs_dir)\n\
             _maturin_dll_patch()\n\
             del _maturin_dll_patch\n\
             # end maturin patch\n"
        ))
    }
}

/// Check if a DLL name is a Windows API set (virtual DLL).
///
/// API sets like `api-ms-win-crt-runtime-l1-1-0.dll` are virtual DLLs that
/// Windows maps to real host DLLs at runtime. They never exist as files on disk.
fn is_api_set_dll(name: &str) -> bool {
    name.starts_with("api-") || name.starts_with("ext-ms-")
}

/// Check if a DLL name matches the Python DLL pattern (pythonXY.dll, python3.dll).
fn is_python_dll(name: &str) -> bool {
    name.starts_with("python3") && name.ends_with(".dll")
}

/// Check if a DLL name matches the Visual C++ runtime redistributable pattern.
fn is_vc_runtime_dll(name: &str) -> bool {
    name.starts_with("vcruntime")
        || name.starts_with("msvcp")
        || name.starts_with("msvcr")
        || name.starts_with("vccorlib")
        || name.starts_with("concrt")
        || name.starts_with("vcamp")
        || name.starts_with("vcomp")
        || name.starts_with("ucrtbase")
        || name.starts_with("mfc")
}

/// Well-known Windows system DLLs that should never be bundled.
///
/// This is a curated fallback list for when path-based detection isn't
/// possible (e.g., cross-compilation, or DLL found via PATH outside the
/// Windows directory).
const KNOWN_SYSTEM_DLLS: &[&str] = &[
    // Core OS
    "kernel32.dll",
    "kernelbase.dll",
    "ntdll.dll",
    "advapi32.dll",
    "user32.dll",
    "gdi32.dll",
    "gdi32full.dll",
    "shell32.dll",
    "ole32.dll",
    "oleaut32.dll",
    "rpcrt4.dll",
    "msvcrt.dll",
    // Networking
    "ws2_32.dll",
    "wsock32.dll",
    "mswsock.dll",
    "winhttp.dll",
    "wininet.dll",
    "iphlpapi.dll",
    "dnsapi.dll",
    "netapi32.dll",
    "wldap32.dll",
    // Security/Crypto
    "secur32.dll",
    "crypt32.dll",
    "bcrypt.dll",
    "bcryptprimitives.dll",
    "ncrypt.dll",
    "wintrust.dll",
    // Shell/UI
    "shlwapi.dll",
    "comctl32.dll",
    "comdlg32.dll",
    "imm32.dll",
    "uxtheme.dll",
    "shcore.dll",
    // System services
    "userenv.dll",
    "dbghelp.dll",
    "psapi.dll",
    "setupapi.dll",
    "cfgmgr32.dll",
    "version.dll",
    "winmm.dll",
    "powrprof.dll",
    "cabinet.dll",
    "msi.dll",
    "imagehlp.dll",
    "normaliz.dll",
    // COM/OLE
    "combase.dll",
    "propsys.dll",
    // Graphics/DirectX
    "d3d9.dll",
    "d3d10.dll",
    "d3d11.dll",
    "d3d12.dll",
    "dxgi.dll",
    "d2d1.dll",
    "d3dcompiler_47.dll",
    "dwrite.dll",
    "opengl32.dll",
    "glu32.dll",
    // WinRT/Modern
    "windowscodecs.dll",
    "mfplat.dll",
    "mf.dll",
    "mfreadwrite.dll",
    // CRT
    "ucrtbase.dll",
];

/// Check if a resolved library path is inside a Windows system directory.
fn is_in_windows_system_dir(realpath: &Path) -> bool {
    let path_str = realpath.to_string_lossy().to_lowercase();
    let path_normalized = path_str.replace('\\', "/");

    let windir = std::env::var("WINDIR")
        .or_else(|_| std::env::var("SystemRoot"))
        .unwrap_or_default()
        .to_lowercase()
        .replace('\\', "/");

    if !windir.is_empty() && path_normalized.starts_with(&format!("{windir}/")) {
        return true;
    }

    // Fallback heuristic for cross-compilation or unusual environments
    path_normalized.contains("/windows/system32/")
        || path_normalized.contains("/windows/syswow64/")
        || path_normalized.contains("/windows/winsxs/")
}

/// Check if a DLL should be excluded from bundling.
///
/// Uses a layered approach:
/// 1. API set DLLs (virtual, never real files) — by name prefix
/// 2. Python DLLs — by name pattern
/// 3. VC runtime redistributables — by name pattern
/// 4. Path-based check — if resolved to a Windows system directory
/// 5. Name-based fallback — curated list for cross-compilation
fn is_system_dll(name: &str, realpath: Option<&Path>) -> bool {
    let lower = name.to_lowercase();

    if is_api_set_dll(&lower) {
        return true;
    }
    if is_python_dll(&lower) {
        return true;
    }
    if is_vc_runtime_dll(&lower) {
        return true;
    }

    if let Some(path) = realpath
        && is_in_windows_system_dir(path)
    {
        return true;
    }

    KNOWN_SYSTEM_DLLS.contains(&lower.as_str())
}

/// Find external DLL dependencies for a Windows artifact.
///
/// Returns DLLs that are NOT system/known DLLs and need to be bundled
/// into the wheel for it to work on other machines.
pub fn find_external_libs(
    artifact: impl AsRef<Path>,
    ld_paths: Vec<PathBuf>,
) -> Result<Vec<lddtree::Library>> {
    let dep_analyzer = lddtree::DependencyAnalyzer::default().library_paths(ld_paths);
    let deps = dep_analyzer.analyze(&artifact).with_context(|| {
        format!(
            "Failed to analyze dependencies for {}",
            artifact.as_ref().display()
        )
    })?;
    let mut ext_libs = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (_name, lib) in deps.libraries {
        if is_system_dll(&lib.name, lib.realpath.as_deref()) {
            continue;
        }
        if !lib.found() {
            continue;
        }
        let lower_name = lib.name.to_lowercase();
        if seen.insert(lower_name) {
            ext_libs.push(lib);
        }
    }
    Ok(ext_libs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_api_set_dlls() {
        assert!(is_api_set_dll("api-ms-win-crt-runtime-l1-1-0.dll"));
        assert!(is_api_set_dll("ext-ms-win-ntuser-uicontext-ext-l1-1-0.dll"));
        assert!(!is_api_set_dll("kernel32.dll"));
    }

    #[test]
    fn test_python_dlls() {
        assert!(is_python_dll("python3.dll"));
        assert!(is_python_dll("python311.dll"));
        assert!(is_python_dll("python312.dll"));
        assert!(!is_python_dll("pythoncom.dll"));
    }

    #[test]
    fn test_vc_runtime_dlls() {
        assert!(is_vc_runtime_dll("vcruntime140.dll"));
        assert!(is_vc_runtime_dll("vcruntime140_1.dll"));
        assert!(is_vc_runtime_dll("vcruntime140d.dll"));
        assert!(is_vc_runtime_dll("msvcp140.dll"));
        assert!(is_vc_runtime_dll("msvcp140_1.dll"));
        assert!(is_vc_runtime_dll("concrt140.dll"));
        assert!(is_vc_runtime_dll("vcomp140.dll"));
        assert!(is_vc_runtime_dll("ucrtbase.dll"));
        assert!(!is_vc_runtime_dll("mylib.dll"));
    }

    #[test]
    fn test_system_dll_by_name() {
        assert!(is_system_dll("kernel32.dll", None));
        assert!(is_system_dll("KERNEL32.DLL", None));
        assert!(is_system_dll("api-ms-win-crt-runtime-l1-1-0.dll", None));
        assert!(is_system_dll("python311.dll", None));
        assert!(is_system_dll("vcruntime140.dll", None));
        assert!(!is_system_dll("libcrypto-3-x64.dll", None));
    }

    #[test]
    fn test_system_dll_by_path() {
        let system32_path = PathBuf::from(r"C:\Windows\System32\obscure_system.dll");
        assert!(is_system_dll("obscure_system.dll", Some(&system32_path)));

        let syswow64_path = PathBuf::from(r"C:\Windows\SysWOW64\another.dll");
        assert!(is_system_dll("another.dll", Some(&syswow64_path)));

        let user_path = PathBuf::from(r"C:\Users\me\libs\mylib.dll");
        assert!(!is_system_dll("mylib.dll", Some(&user_path)));
    }

    #[test]
    fn test_init_py_patch_depth_1() {
        let repairer = WindowsRepairer;
        let patch = repairer.init_py_patch("mypackage.libs", 1).unwrap();
        assert!(patch.contains("# start maturin patch"));
        assert!(patch.contains("# end maturin patch"));
        assert!(patch.contains("os.add_dll_directory(libs_dir)"));
        assert!(patch.contains("hasattr(os, 'add_dll_directory')"));
        assert!(patch.contains(r#"os.pardir, "mypackage.libs""#));
    }

    #[test]
    fn test_init_py_patch_depth_2() {
        let repairer = WindowsRepairer;
        let patch = repairer.init_py_patch("mypackage.libs", 2).unwrap();
        assert!(patch.contains(r#"os.pardir, os.pardir, "mypackage.libs""#));
    }
}
