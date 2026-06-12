//! Platform tag generation for wheel filenames.
//!
//! This module resolves the platform portion of a Python wheel filename
//! (e.g. `manylinux_2_17_x86_64`, `macosx_11_0_arm64`, `win_amd64`) from
//! the build target, environment variables, and project configuration.
//!
//! It also contains the deployment-target / version helpers for macOS, iOS,
//! Emscripten, and Android that feed into the tag generation.

use crate::PyProjectToml;
use crate::auditwheel::PlatformTag;
use crate::target::legacy_py::ALLOWED_PLATFORMS;
use crate::target::{Arch, Os, Target};
use anyhow::{Context, Result, anyhow, bail};
use once_cell::sync::Lazy;
use platform_info::*;
use regex::Regex;
use std::env;
use std::path::Path;

/// Returns the platform portion of a wheel tag for the given target.
///
/// This is a standalone function so that it can be called without a full
/// `BuildContext`.  The `BuildContext::get_platform_tag` method delegates
/// here.
pub fn get_platform_tag(
    target: &Target,
    platform_tags: &[PlatformTag],
    universal2: bool,
    pyproject_toml: Option<&PyProjectToml>,
    manifest_path: &Path,
) -> Result<String> {
    if let Ok(host_platform) = env::var("_PYTHON_HOST_PLATFORM") {
        let override_platform = host_platform.replace(['.', '-'], "_");
        eprintln!(
            "🚉 Overriding platform tag from _PYTHON_HOST_PLATFORM environment variable as {override_platform}."
        );
        return Ok(override_platform);
    }

    let tag = match (&target.target_os(), &target.target_arch()) {
        // Windows
        (Os::Windows, Arch::X86) => "win32".to_string(),
        (Os::Windows, Arch::X86_64) => "win_amd64".to_string(),
        (Os::Windows, Arch::Aarch64) => "win_arm64".to_string(),
        // Android
        (Os::Android, _) => {
            let arch = target.get_platform_arch()?;
            let android_arch = match arch.as_str() {
                "armv7l" => "armeabi_v7a",
                "aarch64" => "arm64_v8a",
                "i686" => "x86",
                "x86_64" => "x86_64",
                _ => bail!("Unsupported Android architecture: {}", arch),
            };
            let api_level = find_android_api_level(target.target_triple(), manifest_path)?;
            format!("android_{}_{}", api_level, android_arch)
        }
        // Linux
        (Os::Linux, _) => {
            let arch = target.get_platform_arch()?;
            let mut platform_tags = platform_tags.to_vec();
            platform_tags.sort();
            let mut tags = vec![];
            for platform_tag in platform_tags {
                tags.push(format!("{platform_tag}_{arch}"));
                for alias in platform_tag.aliases() {
                    let alias_tag = format!("{alias}_{arch}");
                    // Only add legacy aliases if they're in PyPI's static allow-list,
                    // e.g. manylinux2014 was never defined for riscv64
                    if ALLOWED_PLATFORMS.contains(&alias_tag.as_str()) {
                        tags.push(alias_tag);
                    }
                }
            }
            tags.join(".")
        }
        // macOS
        (Os::Macos, Arch::X86_64) | (Os::Macos, Arch::Aarch64) => {
            let ((x86_64_major, x86_64_minor), (arm64_major, arm64_minor)) =
                macosx_deployment_target(
                    env::var("MACOSX_DEPLOYMENT_TARGET").ok().as_deref(),
                    universal2,
                )?;
            let x86_64_tag = if let Some(deployment_target) = pyproject_toml
                .and_then(|x| x.target_config("x86_64-apple-darwin"))
                .and_then(|config| config.macos_deployment_target.as_ref())
            {
                deployment_target.replace('.', "_")
            } else {
                format!("{x86_64_major}_{x86_64_minor}")
            };
            let arm64_tag = if let Some(deployment_target) = pyproject_toml
                .and_then(|x| x.target_config("aarch64-apple-darwin"))
                .and_then(|config| config.macos_deployment_target.as_ref())
            {
                deployment_target.replace('.', "_")
            } else {
                format!("{arm64_major}_{arm64_minor}")
            };
            if universal2 {
                format!(
                    "macosx_{x86_64_tag}_x86_64.macosx_{arm64_tag}_arm64.macosx_{x86_64_tag}_universal2"
                )
            } else if target.target_arch() == Arch::Aarch64 {
                format!("macosx_{arm64_tag}_arm64")
            } else {
                format!("macosx_{x86_64_tag}_x86_64")
            }
        }
        // iOS (simulator and device)
        (Os::Ios, Arch::X86_64) | (Os::Ios, Arch::Aarch64) => {
            let arch = if target.target_arch() == Arch::Aarch64 {
                "arm64"
            } else {
                "x86_64"
            };
            let abi = if target.target_arch() == Arch::X86_64
                || target.target_triple().ends_with("-sim")
            {
                "iphonesimulator"
            } else {
                "iphoneos"
            };
            let (min_sdk_major, min_sdk_minor) = iphoneos_deployment_target(
                env::var("IPHONEOS_DEPLOYMENT_TARGET").ok().as_deref(),
            )?;
            format!("ios_{min_sdk_major}_{min_sdk_minor}_{arch}_{abi}")
        }
        // FreeBSD
        (Os::FreeBsd, _) => {
            format!(
                "{}_{}_{}",
                target.target_os().to_string().to_ascii_lowercase(),
                target.get_platform_release()?.to_ascii_lowercase(),
                target.target_arch().machine(),
            )
        }
        // NetBSD
        (Os::NetBsd, _)
        // OpenBSD
        | (Os::OpenBsd, _) => {
            let release = target.get_platform_release()?;
            format!(
                "{}_{}_{}",
                target.target_os().to_string().to_ascii_lowercase(),
                release,
                target.target_arch().machine(),
            )
        }
        // DragonFly
        (Os::Dragonfly, Arch::X86_64)
        // Haiku
        | (Os::Haiku, Arch::X86_64) => {
            let release = target.get_platform_release()?;
            format!(
                "{}_{}_{}",
                target.target_os().to_string().to_ascii_lowercase(),
                release.to_ascii_lowercase(),
                "x86_64"
            )
        }
        // Emscripten
        (Os::Emscripten, Arch::Wasm32) => emscripten_platform_tag()?,
        (Os::Wasi, Arch::Wasm32) => "any".to_string(),
        // Cygwin
        (Os::Cygwin, _) => {
            format!(
                "{}_{}",
                target.target_os().to_string().to_ascii_lowercase(),
                target.get_platform_arch()?,
            )
        }
        // AIX
        (Os::Aix, _) => aix_platform_tag(target)?,
        // osname_release_machine fallback for any POSIX system
        (_, _) => {
            let info = PlatformInfo::new()
                .map_err(|e| anyhow!("Failed to fetch platform information: {e}"))?;
            let mut release = info.release().to_string_lossy().replace(['.', '-'], "_");
            let mut machine = info.machine().to_string_lossy().replace([' ', '/'], "_");

            let mut os = target.target_os().to_string().to_ascii_lowercase();
            // See https://github.com/python/cpython/blob/46c8d915715aa2bd4d697482aa051fe974d440e1/Lib/sysconfig.py#L722-L730
            if target.target_os() == Os::Solaris || target.target_os() == Os::Illumos {
                // Solaris / Illumos
                if let Some((major, other)) = release.split_once('_') {
                    let major_ver: u64 =
                        major.parse().context("illumos major version is not a number")?;
                    if major_ver >= 5 {
                        // SunOS 5 == Solaris 2
                        os = "solaris".to_string();
                        release = format!("{}_{}", major_ver - 3, other);
                        machine = format!("{machine}_64bit");
                    }
                }
            }
            format!("{os}_{release}_{machine}")
        }
    };
    Ok(tag)
}

/// Get the default macOS deployment target version
fn macosx_deployment_target(
    deploy_target: Option<&str>,
    universal2: bool,
) -> Result<((u16, u16), (u16, u16))> {
    let x86_64_default_rustc = rustc_macosx_target_version("x86_64-apple-darwin");
    let x86_64_default = if universal2 && x86_64_default_rustc.1 < 9 {
        (10, 9)
    } else {
        x86_64_default_rustc
    };
    let arm64_default = rustc_macosx_target_version("aarch64-apple-darwin");
    let mut x86_64_ver = x86_64_default;
    let mut arm64_ver = arm64_default;
    if let Some(deploy_target) = deploy_target {
        let err_ctx = "MACOSX_DEPLOYMENT_TARGET is invalid";
        let mut parts = deploy_target.split('.');
        let major = parts.next().context(err_ctx)?;
        let major: u16 = major.parse().context(err_ctx)?;
        let minor = parts.next().context(err_ctx)?;
        let minor: u16 = minor.parse().context(err_ctx)?;
        if (major, minor) > x86_64_default {
            x86_64_ver = (major, minor);
        }
        if (major, minor) > arm64_default {
            arm64_ver = (major, minor);
        }
    }
    Ok((
        python_macosx_target_version(x86_64_ver),
        python_macosx_target_version(arm64_ver),
    ))
}

/// Get the iOS deployment target version
fn iphoneos_deployment_target(deploy_target: Option<&str>) -> Result<(u16, u16)> {
    let (major, minor) = if let Some(deploy_target) = deploy_target {
        let err_ctx = "IPHONEOS_DEPLOYMENT_TARGET is invalid";
        let mut parts = deploy_target.split('.');
        let major = parts.next().context(err_ctx)?;
        let major: u16 = major.parse().context(err_ctx)?;
        let minor = parts.next().context(err_ctx)?;
        let minor: u16 = minor.parse().context(err_ctx)?;
        (major, minor)
    } else {
        (13, 0)
    };

    Ok((major, minor))
}

#[inline]
fn python_macosx_target_version(version: (u16, u16)) -> (u16, u16) {
    let (major, minor) = version;
    if major >= 11 {
        // pip only supports (major, 0) for macOS 11+
        (major, 0)
    } else {
        (major, minor)
    }
}

/// Query `rustc` for its default macOS deployment target for the given target triple.
///
/// This is also used by `compile.rs` to set `MACOSX_DEPLOYMENT_TARGET` for
/// the cargo build, so it has `pub(crate)` visibility.
pub(crate) fn rustc_macosx_target_version(target: &str) -> (u16, u16) {
    use std::process::{Command, Stdio};
    use target_lexicon::OperatingSystem;

    // On rustc 1.71.0+ we can use `rustc --print deployment-target`
    if let Ok(output) = Command::new("rustc")
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .env_remove("MACOSX_DEPLOYMENT_TARGET")
        .args(["--target", target])
        .args(["--print", "deployment-target"])
        .output()
        && output.status.success()
    {
        let target_version = std::str::from_utf8(&output.stdout)
            .unwrap()
            .split('=')
            .next_back()
            .and_then(|v| v.trim().split_once('.'));
        if let Some((major, minor)) = target_version {
            let major: u16 = major.parse().unwrap();
            let minor: u16 = minor.parse().unwrap();
            return (major, minor);
        }
    }

    let fallback_version = if target == "aarch64-apple-darwin" {
        (11, 0)
    } else {
        (10, 7)
    };

    let rustc_target_version = || -> Result<(u16, u16)> {
        let cmd = Command::new("rustc")
            .arg("-Z")
            .arg("unstable-options")
            .arg("--print")
            .arg("target-spec-json")
            .arg("--target")
            .arg(target)
            .env("RUSTC_BOOTSTRAP", "1")
            .env_remove("MACOSX_DEPLOYMENT_TARGET")
            .output()
            .context("Failed to run rustc to get the target spec")?;
        let stdout = String::from_utf8(cmd.stdout).context("rustc output is not valid utf-8")?;
        let spec: serde_json::Value =
            serde_json::from_str(&stdout).context("rustc output is not valid json")?;
        let llvm_target = spec
            .as_object()
            .context("rustc output is not a json object")?
            .get("llvm-target")
            .context("rustc output does not contain llvm-target")?
            .as_str()
            .context("llvm-target is not a string")?;
        let triple = llvm_target.parse::<target_lexicon::Triple>();
        let (major, minor) = match triple.map(|t| t.operating_system) {
            Ok(
                OperatingSystem::MacOSX(Some(deployment_target))
                | OperatingSystem::Darwin(Some(deployment_target)),
            ) => (deployment_target.major, u16::from(deployment_target.minor)),
            _ => fallback_version,
        };
        Ok((major, minor))
    };
    rustc_target_version().unwrap_or(fallback_version)
}

/// Resolve the platform tag for `wasm32-unknown-emscripten`.
///
/// This implements the priority cascade required to support both
/// [PEP 783](https://peps.python.org/pep-0783/) and older Pyodide config
/// variable names:
///
/// 1. **PEP 783 env override** — emit `pyemscripten_{YEAR}_{PATCH}_wasm32`
///    from `MATURIN_PYEMSCRIPTEN_PLATFORM_VERSION` /
///    `PYEMSCRIPTEN_PLATFORM_VERSION` (the sysconfig variable named by
///    PEP 783).
/// 2. **Historical Pyodide env override** — emit the same PEP 783 tag from
///    `MATURIN_PYODIDE_ABI_VERSION` / `PYODIDE_ABI_VERSION`. Pyodide used to
///    refer to the same platform version as `pyodide_{YEAR}_{PATCH}`, but PyPI
///    only accepts the PEP 783 `pyemscripten_*` platform tag.
/// 3. **Pyodide config auto-detection** — try `pyodide config get
///    pyemscripten_platform_version`, then `pyodide config get
///    pyodide_abi_version`.
/// 4. **Legacy** (Pyodide <= 0.27) — emit
///    `emscripten_{EMCC_VERSION}_wasm32`. Resolved from
///    `MATURIN_EMSCRIPTEN_VERSION` or `emcc -dumpversion`. Emits a warning
///    explaining that the tag is not PEP 783 compliant.
fn emscripten_platform_tag() -> Result<String> {
    if let Some(ver) = first_non_empty_env(&[
        "MATURIN_PYEMSCRIPTEN_PLATFORM_VERSION",
        "PYEMSCRIPTEN_PLATFORM_VERSION",
    ]) {
        return Ok(pep783_emscripten_platform_tag(&ver));
    }
    if let Some(ver) = first_non_empty_env(&["MATURIN_PYODIDE_ABI_VERSION", "PYODIDE_ABI_VERSION"])
    {
        return Ok(pep783_emscripten_platform_tag(&ver));
    }
    if let Some(ver) = pyodide_config_get("pyemscripten_platform_version") {
        return Ok(pep783_emscripten_platform_tag(&ver));
    }
    if let Some(ver) = pyodide_config_get("pyodide_abi_version") {
        return Ok(pep783_emscripten_platform_tag(&ver));
    }
    let release = emscripten_version()?.replace(['.', '-'], "_");
    eprintln!(
        "⚠️  Falling back to legacy `emscripten_{release}_wasm32` platform tag. \
         This wheel will not be installable on PEP 783-compliant Pyodide runtimes. \
         Set `MATURIN_PYEMSCRIPTEN_PLATFORM_VERSION` (PEP 783) or \
         `MATURIN_PYODIDE_ABI_VERSION` (historical Pyodide ABI version env var) \
         to produce a portable tag."
    );
    Ok(format!("emscripten_{release}_wasm32"))
}

fn pep783_emscripten_platform_tag(version: &str) -> String {
    format!("pyemscripten_{version}_wasm32")
}

/// Return the first env var in `names` that is set to a non-empty (after
/// trim) value.
fn first_non_empty_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        env::var(name).ok().and_then(|v| {
            let t = v.trim();
            (!t.is_empty()).then(|| t.to_string())
        })
    })
}

/// Best-effort `pyodide config get <key>` invocation.
///
/// Returns `None` if `pyodide` is not on `PATH`, the command fails, or the
/// reported value is empty / `None`.
fn pyodide_config_get(key: &str) -> Option<String> {
    use std::process::Command;

    let output = Command::new(if cfg!(windows) {
        "pyodide.bat"
    } else {
        "pyodide"
    })
    .arg("config")
    .arg("get")
    .arg(key)
    .output()
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        return None;
    }
    Some(trimmed.to_string())
}

/// Emscripten version
fn emscripten_version() -> Result<String> {
    let os_version = env::var("MATURIN_EMSCRIPTEN_VERSION");
    let release = match os_version {
        Ok(os_ver) => os_ver,
        Err(_) => emcc_version()?,
    };
    Ok(release)
}

fn emcc_version() -> Result<String> {
    use std::process::Command;

    let emcc = Command::new(if cfg!(windows) { "emcc.bat" } else { "emcc" })
        .arg("-dumpversion")
        .output()
        .context("Failed to run emcc to get the version")?;
    let ver = String::from_utf8(emcc.stdout)?;
    let mut trimmed = ver.trim();
    trimmed = trimmed.strip_suffix("-git").unwrap_or(trimmed);
    Ok(trimmed.into())
}

fn extract_android_api_level(value: &str) -> Option<String> {
    static ANDROID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"android(\d+)").unwrap());
    ANDROID_RE.captures(value).map(|caps| caps[1].to_string())
}

fn clang_target_triple() -> Result<String> {
    use std::process::Command;

    let output = Command::new(if cfg!(windows) { "clang.exe" } else { "clang" })
        .arg("-dumpmachine")
        .output()
        .context("Failed to run clang to get the target triple")?;
    let target_triple = String::from_utf8(output.stdout)?;
    Ok(target_triple.trim().to_string())
}

fn find_android_api_level(target_triple: &str, manifest_path: &Path) -> Result<String> {
    if let Ok(val) = env::var("ANDROID_API_LEVEL") {
        return Ok(val);
    }

    let mut clues = Vec::new();

    // 1. Linker from cargo-config2
    if let Some(manifest_dir) = manifest_path.parent()
        && let Ok(config) = cargo_config2::Config::load_with_cwd(manifest_dir)
        && let Ok(Some(linker)) = config.linker(target_triple)
    {
        clues.push(linker.to_string_lossy().into_owned());
    }

    // 2. CC env vars
    if let Ok(cc) = env::var(format!("CC_{}", target_triple.replace('-', "_"))) {
        clues.push(cc);
    }
    if let Ok(cc) = env::var("CC") {
        clues.push(cc);
    }

    for clue in clues {
        if let Some(api_level) = extract_android_api_level(&clue) {
            return Ok(api_level);
        }
    }

    // 3. Check if running on Android (e.g. Termux), then use clang's default target triple
    let is_android = PlatformInfo::new()
        .map(|info| info.release().to_string_lossy().contains("android"))
        .unwrap_or(false);
    if is_android
        && let Ok(target_triple) = clang_target_triple()
        && let Some(api_level) = extract_android_api_level(&target_triple)
    {
        return Ok(api_level);
    }

    bail!(
        "Failed to determine Android API level. Please set the ANDROID_API_LEVEL environment variable."
    );
}

/// Returns the AIX platform tag, matching CPython's `_aix_support.aix_platform()`.
///
/// The tag format is `aix_{ver:x}{rel}{tl:02}_{builddate:04}_{bitsize}`, e.g.
/// `aix_7302_2419_64` for AIX 7.3 TL02 build-week 2419, 64-bit.
///
/// The version, technology level, and build date are read from
/// `/usr/bin/lslpp -Lqc bos.rte`, just like CPython does in
/// [`_aix_support.py`](https://github.com/python/cpython/blob/main/Lib/_aix_support.py).
///
/// The tag produced here matches what `sysconfig.get_platform()` returns on the
/// same host, so wheels saved with this tag are accepted by pip.
fn aix_platform_tag(target: &Target) -> Result<String> {
    use std::process::Command;

    // When cross-compiling for AIX from another host, /usr/bin/lslpp is not
    // available. The caller must set _PYTHON_HOST_PLATFORM (checked before
    // this branch is reached) to supply the correct tag manually.
    if target.cross_compiling() {
        bail!(
            "Cannot determine the AIX platform tag when cross-compiling. \
             Set the _PYTHON_HOST_PLATFORM environment variable to the target \
             platform tag (e.g. `aix_7302_2419_64`)."
        );
    }

    let output = Command::new("/usr/bin/lslpp")
        .args(["-Lqc", "bos.rte"])
        .output()
        .context("Failed to run /usr/bin/lslpp")?;

    if !output.status.success() {
        bail!(
            "lslpp -Lqc bos.rte failed (exit {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }

    let stdout = String::from_utf8(output.stdout).context("lslpp output is not valid UTF-8")?;
    aix_tag_from_lslpp_output(&stdout)
}

/// Parse `lslpp -Lqc bos.rte` output and return the AIX platform tag.
///
/// Extracted from [`aix_platform_tag`] so it can be unit-tested without
/// requiring a real AIX system.
///
/// Output format: `filesystem:name:level:state:type:description:...:builddate`
/// where `level` (field 2) is a VRMF string like `"7.3.2.0"` and `builddate`
/// (last field) is a 4-digit build-week number like `"2419"`.
fn aix_tag_from_lslpp_output(stdout: &str) -> Result<String> {
    let line = stdout.lines().next().unwrap_or("").trim();
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() < 3 {
        bail!("Unexpected lslpp output: {line:?}");
    }

    let vrmf = fields[2]; // e.g. "7.3.2.0"
    // builddate is the last colon-separated field, e.g. "2419".
    // Matches CPython's `int(out[-1]) if out[-1] != '' else 9988`.
    let builddate: u32 = fields
        .last()
        .and_then(|f| f.trim().parse().ok())
        .unwrap_or(9988);

    // Parse V.R.M from VRMF (V=version, R=release, M=technology level)
    let parts: Vec<&str> = vrmf.splitn(4, '.').collect();
    if parts.len() < 3 {
        bail!("Unexpected VRMF format from lslpp: {vrmf:?}");
    }
    let ver: u32 = parts[0].parse().context("VRMF version is not a number")?;
    let rel: u32 = parts[1].parse().context("VRMF release is not a number")?;
    let tl: u32 = parts[2]
        .parse()
        .context("VRMF technology level is not a number")?;

    let bitsize = 64u32; // only 64-bit is supported

    // Format matching CPython's _aix_tag():
    //   "aix-{v:1x}{r:1d}{tl:02d}-{bd:04d}-{sz}" with '-' replaced by '_'
    Ok(format!("aix_{ver:x}{rel}{tl:02}_{builddate:04}_{bitsize}"))
}

#[cfg(test)]
mod tests {
    use super::{
        aix_tag_from_lslpp_output, emscripten_platform_tag, extract_android_api_level,
        iphoneos_deployment_target, macosx_deployment_target, pep783_emscripten_platform_tag,
    };
    use pretty_assertions::assert_eq;
    use std::env;
    use std::ffi::OsString;

    struct EnvVarRestore {
        vars: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvVarRestore {
        fn new(names: &[&'static str]) -> Self {
            let vars = names
                .iter()
                .map(|&name| (name, env::var_os(name)))
                .collect();
            Self { vars }
        }
    }

    impl Drop for EnvVarRestore {
        fn drop(&mut self) {
            for (name, value) in &self.vars {
                unsafe {
                    if let Some(value) = value {
                        env::set_var(name, value);
                    } else {
                        env::remove_var(name);
                    }
                }
            }
        }
    }

    #[test]
    fn test_pep783_emscripten_platform_tag() {
        assert_eq!(
            pep783_emscripten_platform_tag("2025_0"),
            "pyemscripten_2025_0_wasm32"
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_emscripten_platform_tag_uses_pyodide_abi_version_for_pep783() {
        let _guard = EnvVarRestore::new(&[
            "MATURIN_PYEMSCRIPTEN_PLATFORM_VERSION",
            "PYEMSCRIPTEN_PLATFORM_VERSION",
            "MATURIN_PYODIDE_ABI_VERSION",
            "PYODIDE_ABI_VERSION",
        ]);
        unsafe {
            env::remove_var("MATURIN_PYEMSCRIPTEN_PLATFORM_VERSION");
            env::remove_var("PYEMSCRIPTEN_PLATFORM_VERSION");
            env::set_var("MATURIN_PYODIDE_ABI_VERSION", "2025_0");
            env::remove_var("PYODIDE_ABI_VERSION");
        }

        assert_eq!(
            emscripten_platform_tag().unwrap(),
            "pyemscripten_2025_0_wasm32"
        );
    }

    #[test]
    fn test_macosx_deployment_target() {
        let rustc_ver = rustc_version::version().unwrap();
        let rustc_ver = (rustc_ver.major, rustc_ver.minor);
        let x86_64_minor = if rustc_ver >= (1, 74) { 12 } else { 7 };
        let universal2_minor = if rustc_ver >= (1, 74) { 12 } else { 9 };
        assert_eq!(
            macosx_deployment_target(None, false).unwrap(),
            ((10, x86_64_minor), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(None, true).unwrap(),
            ((10, universal2_minor), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.6"), false).unwrap(),
            ((10, x86_64_minor), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.6"), true).unwrap(),
            ((10, universal2_minor), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.9"), false).unwrap(),
            ((10, universal2_minor), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("11.0.0"), false).unwrap(),
            ((11, 0), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("11.1"), false).unwrap(),
            ((11, 0), (11, 0))
        );
    }

    #[test]
    fn test_iphoneos_deployment_target() {
        // Use default when no value is provided
        assert_eq!(iphoneos_deployment_target(None).unwrap(), (13, 0));

        // Valid version strings
        assert_eq!(iphoneos_deployment_target(Some("13.0")).unwrap(), (13, 0));
        assert_eq!(iphoneos_deployment_target(Some("14.5")).unwrap(), (14, 5));
        assert_eq!(iphoneos_deployment_target(Some("15.0")).unwrap(), (15, 0));
        // Extra parts are ignored
        assert_eq!(iphoneos_deployment_target(Some("14.5.1")).unwrap(), (14, 5));

        // Invalid formats
        assert!(iphoneos_deployment_target(Some("invalid")).is_err());
        assert!(iphoneos_deployment_target(Some("13")).is_err());
        assert!(iphoneos_deployment_target(Some("13.")).is_err());
        assert!(iphoneos_deployment_target(Some(".0")).is_err());
        assert!(iphoneos_deployment_target(Some("abc.def")).is_err());
        assert!(iphoneos_deployment_target(Some("13.abc")).is_err());
        assert!(iphoneos_deployment_target(Some("")).is_err());
    }

    #[test]
    fn test_extract_android_api_level() {
        assert_eq!(
            extract_android_api_level("aarch64-linux-android24-clang"),
            Some("24".to_string())
        );
        assert_eq!(
            extract_android_api_level("aarch64-unknown-linux-android30"),
            Some("30".to_string())
        );
        assert_eq!(extract_android_api_level("clang"), None);
    }

    #[test]
    fn test_aix_tag_from_lslpp_output_typical() {
        // Typical lslpp -Lqc bos.rte output on AIX 7.3 TL02 SP01, build-week 2419.
        // Format: filesystem:name:level:state:type:description:...:builddate
        let output =
            "/usr/lib/objrepos:bos.rte:7.3.2.1:COMMITTED:I:Base Operating System Runtime:2419\n";
        assert_eq!(
            aix_tag_from_lslpp_output(output).unwrap(),
            "aix_7302_2419_64"
        );
    }

    #[test]
    fn test_aix_tag_from_lslpp_output_aix61() {
        // AIX 6.1 TL07, build-week 1415, 64-bit — matches CPython docs example.
        let output =
            "/usr/lib/objrepos:bos.rte:6.1.7.2:COMMITTED:I:Base Operating System Runtime:1415\n";
        assert_eq!(
            aix_tag_from_lslpp_output(output).unwrap(),
            "aix_6107_1415_64"
        );
    }

    #[test]
    fn test_aix_tag_from_lslpp_output_tl_zero_padded() {
        // TL single digit must be zero-padded to two digits in the tag.
        let output =
            "/usr/lib/objrepos:bos.rte:7.3.1.0:COMMITTED:I:Base Operating System Runtime:2312\n";
        assert_eq!(
            aix_tag_from_lslpp_output(output).unwrap(),
            "aix_7301_2312_64"
        );
    }

    #[test]
    fn test_aix_tag_from_lslpp_output_missing_builddate_falls_back() {
        // When the builddate field is absent or non-numeric, CPython falls back to 9988.
        let output =
            "/usr/lib/objrepos:bos.rte:7.3.2.0:COMMITTED:I:Base Operating System Runtime:\n";
        assert_eq!(
            aix_tag_from_lslpp_output(output).unwrap(),
            "aix_7302_9988_64"
        );
    }

    #[test]
    fn test_aix_tag_from_lslpp_output_extra_fields_before_builddate() {
        // Additional colon-separated fields between description and builddate
        // are fine; builddate is always the last field.
        let output =
            "/usr/lib/objrepos:bos.rte:7.3.2.0:COMMITTED:I:Base OS Runtime:extra:field:2419\n";
        assert_eq!(
            aix_tag_from_lslpp_output(output).unwrap(),
            "aix_7302_2419_64"
        );
    }

    #[test]
    fn test_aix_tag_from_lslpp_output_too_few_fields_is_error() {
        assert!(aix_tag_from_lslpp_output("bos.rte:7.3.2.0\n").is_err());
        assert!(aix_tag_from_lslpp_output("").is_err());
    }

    #[test]
    fn test_aix_tag_from_lslpp_output_bad_vrmf_is_error() {
        // VRMF with fewer than three components.
        let output =
            "/usr/lib/objrepos:bos.rte:7.3:COMMITTED:I:Base Operating System Runtime:2419\n";
        assert!(aix_tag_from_lslpp_output(output).is_err());
    }

    #[test]
    fn test_aix_tag_from_lslpp_output_non_numeric_vrmf_is_error() {
        let output =
            "/usr/lib/objrepos:bos.rte:x.y.z.0:COMMITTED:I:Base Operating System Runtime:2419\n";
        assert!(aix_tag_from_lslpp_output(output).is_err());
    }
}
