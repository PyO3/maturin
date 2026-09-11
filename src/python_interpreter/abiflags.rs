//! ABI flag resolution and ABI tag calculation.
//!
//! This module handles the platform-specific logic for determining Python ABI flags
//! (e.g. `"m"`, `"t"`, `""`) and computing ABI tags from extension suffixes.

use crate::{BridgeModel, Target};
use anyhow::{Result, bail, ensure};

use super::discovery::InterpreterMetadataMessage;

pub(super) fn default_abiflags(minor_version: usize, gil_disabled: bool, debug: bool) -> String {
    let mut abiflags = String::new();

    // order is [t][d][m], to match order used in `packaging`
    // https://github.com/pypa/packaging/blob/10590c194edb33c82f84a127883d6097c56b7840/src/packaging/tags.py#L376-L390
    if gil_disabled {
        abiflags.push('t');
    }

    if debug {
        abiflags.push('d');
    }

    // pymalloc abi was the default until 3.8
    if minor_version < 8 {
        abiflags.push('m');
    }

    abiflags
}

pub(super) fn validate_abiflags(abiflags: &str, gil_disabled: bool, debug: bool) -> Result<()> {
    for (flag, enabled, name) in [
        ('t', gil_disabled, "Py_GIL_DISABLED"),
        ('d', debug, "Py_DEBUG"),
    ] {
        ensure!(
            abiflags.contains(flag) == enabled,
            "ABI flags are inconsistent with {name}={enabled}"
        );
    }
    Ok(())
}

/// Returns the abiflags that are assembled through the message, with some
/// additional sanity checks.
///
/// The rules are as follows:
///  - python 3 + Unix: Use ABIFLAGS
///  - python 3 + Windows: Use ABIFLAGS when available, otherwise infer them
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
            Some("") | None => Ok(default_abiflags(
                message.minor,
                message.gil_disabled,
                message.debug,
            )),
            Some(abiflags) => {
                validate_abiflags(abiflags, message.gil_disabled, message.debug)?;
                Ok(abiflags.to_string())
            }
        }
    } else if let Some(abiflags) = &message.abiflags {
        validate_abiflags(abiflags, message.gil_disabled, message.debug)?;
        Ok(abiflags.to_string())
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
        soabi_split.nth(1)?.to_string()
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
    fn test_default_abiflags() {
        for (minor_version, gil_disabled, debug, expected) in [
            (7, false, false, "m"),
            (7, false, true, "dm"),
            (8, false, false, ""),
            (8, false, true, "d"),
            (13, false, false, ""),
            (13, true, false, "t"),
            (13, true, true, "td"),
        ] {
            let flags = default_abiflags(minor_version, gil_disabled, debug);
            assert_eq!(flags, expected);
        }
    }

    #[test]
    fn test_validate_abiflags() {
        for (flags, debug, gil_disabled) in [
            ("", false, false),
            ("d", true, false),
            ("t", false, true),
            ("td", true, true),
        ] {
            assert!(validate_abiflags(flags, gil_disabled, debug,).is_ok());
            assert!(validate_abiflags(flags, !gil_disabled, debug,).is_err());
            assert!(validate_abiflags(flags, gil_disabled, !debug).is_err());
        }
    }

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
