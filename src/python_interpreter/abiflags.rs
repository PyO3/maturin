//! ABI flag resolution and ABI tag calculation.
//!
//! This module handles the platform-specific logic for determining Python ABI flags
//! (e.g. `"m"`, `"t"`, `""`) and computing ABI tags from extension suffixes.

use crate::{BridgeModel, Target};
use anyhow::{Result, bail, ensure};

use super::discovery::InterpreterMetadataMessage;

/// Returns the abiflags that are assembled through the message, with some
/// additional sanity checks.
///
/// The rules are as follows:
///  - python 3 + Unix: Use ABIFLAGS
///  - python 3 + Windows: No ABIFLAGS, return an empty string
pub(super) fn fun_with_abiflags(
    message: &InterpreterMetadataMessage,
    target: &Target,
    bridge: &BridgeModel,
) -> Result<String> {
    if bridge != &BridgeModel::Cffi
        && target.get_python_os() != message.system
        && !target.cross_compiling()
        && !(target.get_python_os() == "cygwin"
            && message.system.to_lowercase().starts_with("cygwin"))
    {
        // Python versions <= 3.12 used to report platform.system() as "linux". Only on Python versions
        // >= 3.13, platform.system() reports as "android". So maintain backwards compatibility with
        // Python 3.12 when compiling on Android environment (for e.g. Termux)
        let is_android_compat = target.get_python_os() == "android"
            && message.system == "linux"
            && message.major == 3
            && message.minor <= 12;
        if !is_android_compat {
            bail!(
                "platform.system() in python, {}, and the rust target, {:?}, don't match ಠ_ಠ",
                message.system,
                target,
            )
        }
    }

    if message.major != 3 || message.minor < 7 {
        bail!(
            "Only python >= 3.7 is supported, while you're using python {}.{}",
            message.major,
            message.minor
        );
    }

    if matches!(message.interpreter.as_str(), "pypy" | "graalvm" | "graalpy") {
        // pypy and graalpy do not specify abi flags
        Ok("".to_string())
    } else if message.system == "windows" {
        // On Windows:
        // - Python < 3.8: abiflags is empty/None but we need "m"
        // - Python 3.8 - 3.13: abiflags is empty/None
        // - Python 3.13t: abiflags is empty/None but we need "t" (gil_disabled)
        // - Python >= 3.14: abiflags is now defined in sysconfig (upstream change)
        match message.abiflags.as_deref() {
            Some("") | None => {
                if message.minor <= 7 {
                    Ok("m".to_string())
                } else if message.gil_disabled {
                    ensure!(
                        message.minor >= 13,
                        "gil_disabled is only available in python 3.13+ ಠ_ಠ"
                    );
                    Ok("t".to_string())
                } else {
                    Ok("".to_string())
                }
            }
            Some(abiflags) => {
                // Python 3.14+ on Windows now defines ABIFLAGS in sysconfig.
                // Accept it (fixes #2740).
                if message.minor >= 14 {
                    Ok(abiflags.to_string())
                } else if message.gil_disabled && abiflags == "t" {
                    // Python 3.13t may also report "t"
                    Ok(abiflags.to_string())
                } else {
                    bail!(
                        "Unexpected abiflags '{}' for Python {}.{} on Windows ಠ_ಠ",
                        abiflags,
                        message.major,
                        message.minor
                    )
                }
            }
        }
    } else if let Some(ref abiflags) = message.abiflags {
        if message.minor >= 8 {
            // for 3.8, "builds with and without pymalloc are ABI compatible" and the flag dropped
            Ok(abiflags.to_string())
        } else if (abiflags != "m") && (abiflags != "dm") {
            bail!("A python 3 interpreter on Linux or macOS must have 'm' or 'dm' as abiflags ಠ_ಠ")
        } else {
            Ok(abiflags.to_string())
        }
    } else {
        bail!("A python 3 interpreter on Linux or macOS must define abiflags in its sysconfig ಠ_ಠ")
    }
}

/// Calculate the ABI tag from EXT_SUFFIX
pub(super) fn calculate_abi_tag(ext_suffix: &str) -> Option<String> {
    let parts = ext_suffix.split('.').collect::<Vec<_>>();
    if parts.len() < 3 {
        // CPython3.7 and earlier uses ".pyd" on Windows.
        return None;
    }
    let soabi = parts[1];
    let mut soabi_split = soabi.split('-');
    let abi = if soabi.starts_with("cpython") {
        // non-windows
        format!("cp{}", soabi_split.nth(1).unwrap())
    } else if soabi.starts_with("cp") {
        // windows
        soabi_split.next().unwrap().to_string()
    } else if soabi.starts_with("pypy") {
        soabi_split.take(2).collect::<Vec<_>>().join("-")
    } else if soabi.starts_with("graalpy") {
        soabi_split.take(3).collect::<Vec<_>>().join("-")
    } else if !soabi.is_empty() {
        // pyston, ironpython, others?
        match soabi_split.nth(1) {
            Some(abi) => abi.to_string(),
            None => return None,
        }
    } else {
        return None;
    };
    let abi_tag = abi.replace(['.', '-', ' '], "_");
    Some(abi_tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_abi_tag() {
        let cases = vec![
            (".cpython-37m-x86_64-linux-gnu.so", Some("cp37m")),
            (".cpython-310-x86_64-linux-gnu.so", Some("cp310")),
            (".cpython-310-darwin.so", Some("cp310")),
            (".cpython-313t-darwin.so", Some("cp313t")),
            (".cp310-win_amd64.pyd", Some("cp310")),
            (".cp39-mingw_x86_64.pyd", Some("cp39")),
            (".cpython-312-wasm32-wasi.so", Some("cp312")),
            (".cpython-38.so", Some("cp38")),
            (".pyd", None),
            (".so", None),
            (".pypy38-pp73-x86_64-linux-gnu.so", Some("pypy38_pp73")),
            (
                ".graalpy-38-native-x86_64-darwin.dylib",
                Some("graalpy_38_native"),
            ),
            (".pyston-23-x86_64-linux-gnu.so", Some("23")),
            // soabi without dashes should return None, not panic
            (".nodashes.so", None),
        ];
        for (ext_suffix, expected) in cases {
            assert_eq!(calculate_abi_tag(ext_suffix).as_deref(), expected);
        }
    }
}
