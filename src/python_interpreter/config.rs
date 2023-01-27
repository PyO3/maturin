use super::InterpreterKind;
use crate::target::{Arch, Os};
use crate::Target;
use anyhow::{format_err, Context, Result};
use fs_err as fs;
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Wellknown Python interpreter sysconfig values
static WELLKNOWN_SYSCONFIG: Lazy<HashMap<Os, HashMap<Arch, Vec<InterpreterConfig>>>> =
    Lazy::new(|| {
        let mut sysconfig = HashMap::new();
        // Linux
        let sysconfig_linux = serde_json::from_slice(include_bytes!("sysconfig-linux.json"))
            .expect("invalid sysconfig-linux.json");
        sysconfig.insert(Os::Linux, sysconfig_linux);
        // macOS
        let sysconfig_macos = serde_json::from_slice(include_bytes!("sysconfig-macos.json"))
            .expect("invalid sysconfig-macos.json");
        sysconfig.insert(Os::Macos, sysconfig_macos);
        // Windows
        let sysconfig_windows = serde_json::from_slice(include_bytes!("sysconfig-windows.json"))
            .expect("invalid sysconfig-windows.json");
        sysconfig.insert(Os::Windows, sysconfig_windows);
        // FreeBSD
        let sysconfig_freebsd = serde_json::from_slice(include_bytes!("sysconfig-freebsd.json"))
            .expect("invalid sysconfig-freebsd.json");
        sysconfig.insert(Os::FreeBsd, sysconfig_freebsd);
        // OpenBSD
        let sysconfig_openbsd = serde_json::from_slice(include_bytes!("sysconfig-openbsd.json"))
            .expect("invalid sysconfig-openbsd.json");
        sysconfig.insert(Os::OpenBsd, sysconfig_openbsd);
        // NetBSD
        let sysconfig_netbsd = serde_json::from_slice(include_bytes!("sysconfig-netbsd.json"))
            .expect("invalid sysconfig-netbsd.json");
        sysconfig.insert(Os::NetBsd, sysconfig_netbsd);
        // Emscripten
        let sysconfig_emscripten =
            serde_json::from_slice(include_bytes!("sysconfig-emscripten.json"))
                .expect("invalid sysconfig-emscripten.json");
        sysconfig.insert(Os::Emscripten, sysconfig_emscripten);
        sysconfig
    });

/// Some of the sysconfigdata of Python interpreter we care about
#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
pub struct InterpreterConfig {
    /// Python's major version
    pub major: usize,
    /// Python's minor version
    pub minor: usize,
    /// cpython or pypy
    #[serde(rename = "interpreter")]
    pub interpreter_kind: InterpreterKind,
    /// For linux and mac, this contains the value of the abiflags, e.g. "m"
    /// for python3.7m or "dm" for python3.6dm. Since python3.8, the value is
    /// empty. On windows, the value was always None.
    ///
    /// See PEP 261 and PEP 393 for details
    pub abiflags: String,
    /// Suffix to use for extension modules as given by sysconfig.
    pub ext_suffix: String,
    /// Part of sysconfig's SOABI specifying {major}{minor}{abiflags}
    ///
    /// Note that this always `None` on windows
    pub abi_tag: Option<String>,
    /// Pointer width
    pub pointer_width: Option<usize>,
}

impl InterpreterConfig {
    /// Lookup a wellknown sysconfig for a given Python interpreter
    pub fn lookup(
        os: Os,
        arch: Arch,
        python_impl: InterpreterKind,
        python_version: (usize, usize),
    ) -> Option<&'static Self> {
        let (major, minor) = python_version;
        if let Some(os_sysconfigs) = WELLKNOWN_SYSCONFIG.get(&os) {
            if let Some(sysconfigs) = os_sysconfigs.get(&arch) {
                return sysconfigs.iter().find(|s| {
                    s.interpreter_kind == python_impl && s.major == major && s.minor == minor
                });
            }
        }
        None
    }

    /// Lookup wellknown sysconfigs for a given target
    pub fn lookup_target(target: &Target) -> Vec<Self> {
        if let Some(os_sysconfigs) = WELLKNOWN_SYSCONFIG.get(&target.target_os()) {
            if let Some(sysconfigs) = os_sysconfigs.get(&target.target_arch()).cloned() {
                return sysconfigs;
            }
        }
        Vec::new()
    }

    /// Construct a new InterpreterConfig from a pyo3 config file
    pub fn from_pyo3_config(config_file: &Path, target: &Target) -> Result<Self> {
        let config_file = fs::File::open(config_file)?;
        let reader = BufReader::new(config_file);
        let lines = reader.lines();

        macro_rules! parse_value {
            ($variable:ident, $value:ident) => {
                $variable = Some($value.trim().parse().context(format!(
                    concat!(
                        "failed to parse ",
                        stringify!($variable),
                        " from config value '{}'"
                    ),
                    $value
                ))?)
            };
        }

        let mut implementation = None;
        let mut version = None;
        let mut abiflags = None;
        let mut ext_suffix = None;
        let mut abi_tag = None;
        let mut pointer_width = None;

        for (i, line) in lines.enumerate() {
            let line = line.context("failed to read line from config")?;
            let (key, value) = line
                .split_once('=')
                .with_context(|| format!("expected key=value pair on line {}", i + 1))?;
            match key {
                "implementation" => parse_value!(implementation, value),
                "version" => parse_value!(version, value),
                "abiflags" => parse_value!(abiflags, value),
                "ext_suffix" => parse_value!(ext_suffix, value),
                "abi_tag" => parse_value!(abi_tag, value),
                "pointer_width" => parse_value!(pointer_width, value),
                _ => continue,
            }
        }
        let version: String = version.context("missing value for version")?;
        let (ver_major, ver_minor) = version
            .split_once('.')
            .context("Invalid python interpreter version")?;
        let major = ver_major.parse::<usize>().with_context(|| {
            format!("Invalid python interpreter major version '{ver_major}', expect a digit")
        })?;
        let minor = ver_minor.parse::<usize>().with_context(|| {
            format!("Invalid python interpreter minor version '{ver_minor}', expect a digit")
        })?;
        let implementation = implementation.unwrap_or_else(|| "cpython".to_string());
        let interpreter_kind = implementation.parse().map_err(|e| format_err!("{}", e))?;
        let abi_tag = match interpreter_kind {
            InterpreterKind::CPython => {
                if (major, minor) >= (3, 8) {
                    abi_tag.unwrap_or_else(|| format!("{major}{minor}"))
                } else {
                    abi_tag.unwrap_or_else(|| format!("{major}{minor}m"))
                }
            }
            InterpreterKind::PyPy => abi_tag.unwrap_or_else(|| "pp73".to_string()),
        };
        let file_ext = if target.is_windows() { "pyd" } else { "so" };
        let ext_suffix = if target.is_linux() || target.is_macos() {
            // See https://github.com/pypa/auditwheel/issues/349
            let target_env = if (major, minor) >= (3, 11) {
                target.target_env().to_string()
            } else {
                "gnu".to_string()
            };
            match interpreter_kind {
                InterpreterKind::CPython => ext_suffix.unwrap_or_else(|| {
                    // Eg: .cpython-38-x86_64-linux-gnu.so
                    format!(
                        ".cpython-{}-{}-{}-{}.{}",
                        abi_tag,
                        target.get_python_arch(),
                        target.get_python_os(),
                        target_env,
                        file_ext,
                    )
                }),
                InterpreterKind::PyPy => ext_suffix.unwrap_or_else(|| {
                    // Eg: .pypy38-pp73-x86_64-linux-gnu.so
                    format!(
                        ".pypy{}{}-{}-{}-{}-{}.{}",
                        major,
                        minor,
                        abi_tag,
                        target.get_python_arch(),
                        target.get_python_os(),
                        target_env,
                        file_ext,
                    )
                }),
            }
        } else {
            ext_suffix.context("missing value for ext_suffix")?
        };
        Ok(Self {
            major,
            minor,
            interpreter_kind,
            abiflags: abiflags.unwrap_or_default(),
            ext_suffix,
            abi_tag: Some(abi_tag),
            pointer_width,
        })
    }

    /// Generate pyo3 config file content
    pub fn pyo3_config_file(&self) -> String {
        let mut content = format!(
            r#"implementation={implementation}
version={major}.{minor}
shared=true
abi3=false
build_flags=WITH_THREAD
suppress_build_script_link_lines=false"#,
            implementation = self.interpreter_kind,
            major = self.major,
            minor = self.minor,
        );
        if let Some(pointer_width) = self.pointer_width {
            write!(content, "\npointer_width={pointer_width}").unwrap();
        }
        content
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_load_sysconfig() {
        let linux_sysconfig = WELLKNOWN_SYSCONFIG.get(&Os::Linux).unwrap();
        assert!(linux_sysconfig.contains_key(&Arch::X86_64));
    }

    #[test]
    fn test_pyo3_config_file() {
        let sysconfig =
            InterpreterConfig::lookup(Os::Linux, Arch::X86_64, InterpreterKind::CPython, (3, 10))
                .unwrap();
        let config_file = sysconfig.pyo3_config_file();
        assert_eq!(config_file, "implementation=CPython\nversion=3.10\nshared=true\nabi3=false\nbuild_flags=WITH_THREAD\nsuppress_build_script_link_lines=false\npointer_width=64");
    }
}
