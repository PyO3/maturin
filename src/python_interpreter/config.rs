use super::InterpreterKind;
use crate::target::{Arch, Os};
use anyhow::{format_err, Context, Result};
use fs_err as fs;
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Wellknown Python interpreter sysconfig values
static WELLKNOWN_SYSCONFIG: Lazy<HashMap<Os, HashMap<Arch, Vec<InterpreterConfig>>>> =
    Lazy::new(|| {
        let mut sysconfig = HashMap::new();
        let sysconfig_linux = serde_json::from_slice(include_bytes!("sysconfig-linux.json"))
            .expect("invalid sysconfig-linux.json");
        sysconfig.insert(Os::Linux, sysconfig_linux);
        let sysconfig_macos = serde_json::from_slice(include_bytes!("sysconfig-macos.json"))
            .expect("invalid sysconfig-macos.json");
        sysconfig.insert(Os::Macos, sysconfig_macos);
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

    /// Construct a new InterpreterConfig from a pyo3 config file
    pub fn from_pyo3_config(config_file: &Path) -> Result<Self> {
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
            format!(
                "Invalid python interpreter major version '{}', expect a digit",
                ver_major
            )
        })?;
        let minor = ver_minor.parse::<usize>().with_context(|| {
            format!(
                "Invalid python interpreter minor version '{}', expect a digit",
                ver_minor
            )
        })?;
        let implementation = implementation.unwrap_or_else(|| "cpython".to_string());
        let interpreter_kind = implementation.parse().map_err(|e| format_err!("{}", e))?;
        let ext_suffix = ext_suffix.context("missing value for ext_suffix")?;
        Ok(Self {
            major,
            minor,
            interpreter_kind,
            abiflags: abiflags.unwrap_or_default(),
            ext_suffix,
            abi_tag,
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
            content.push_str(&format!("\npointer_width={}", pointer_width));
        }
        content
    }
}

#[cfg(test)]
mod test {
    use super::*;

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
