use super::InterpreterKind;
use crate::target::{Arch, Os};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;

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
        // FreeBSD
        let sysconfig_freebsd = serde_json::from_slice(include_bytes!("sysconfig-freebsd.json"))
            .expect("invalid sysconfig-freebsd.json");
        sysconfig.insert(Os::FreeBsd, sysconfig_freebsd);
        // OpenBSD
        let sysconfig_openbsd = serde_json::from_slice(include_bytes!("sysconfig-openbsd.json"))
            .expect("invalid sysconfig-openbsd.json");
        sysconfig.insert(Os::OpenBsd, sysconfig_openbsd);
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
