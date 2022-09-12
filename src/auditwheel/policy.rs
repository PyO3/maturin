use crate::auditwheel::PlatformTag;
use crate::target::Arch;
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::cmp::{Ordering, PartialOrd};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::{Display, Formatter};

/// The policies (allowed symbols) for the different manylinux tags, sorted from highest
/// priority to lowest
pub static MANYLINUX_POLICIES: Lazy<Vec<Policy>> = Lazy::new(|| {
    // https://github.com/pypa/auditwheel/blob/master/auditwheel/policy/manylinux-policy.json
    let mut policies: Vec<Policy> = serde_json::from_slice(include_bytes!("manylinux-policy.json"))
        .expect("invalid manylinux policy.json file");
    policies.sort_by_key(|policy| -policy.priority);
    policies
});

/// The policies (allowed symbols) for the different musllinux tags, sorted from highest
/// priority to lowest
pub static MUSLLINUX_POLICIES: Lazy<Vec<Policy>> = Lazy::new(|| {
    // https://github.com/pypa/auditwheel/blob/master/auditwheel/policy/musllinux-policy.json
    let mut policies: Vec<Policy> = serde_json::from_slice(include_bytes!("musllinux-policy.json"))
        .expect("invalid musllinux policy.json file");
    policies.sort_by_key(|policy| -policy.priority);
    policies
});

/// Manylinux policy
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Policy {
    /// platform tag name
    pub name: String,
    /// platform tag aliases
    pub aliases: Vec<String>,
    /// policy priority. Tags supporting more platforms have higher priority
    pub priority: i64,
    /// platform architecture to symbol versions map
    #[serde(rename = "symbol_versions")]
    pub symbol_versions: HashMap<String, HashMap<String, HashSet<String>>>,
    /// whitelisted libraries
    #[serde(rename = "lib_whitelist")]
    pub lib_whitelist: HashSet<String>,
    /// blacklisted symbols of whitelisted libraries
    pub blacklist: HashMap<String, HashSet<String>>,
}

impl Default for Policy {
    fn default() -> Self {
        // defaults to linux
        Policy::from_name("linux").unwrap()
    }
}

impl Display for Policy {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.aliases.is_empty() {
            f.write_str(&self.name)
        } else {
            f.write_fmt(format_args!(
                "{} (aka {})",
                &self.name,
                self.aliases.join(",")
            ))
        }
    }
}

impl PartialOrd for Policy {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.priority.partial_cmp(&other.priority)
    }
}

impl Policy {
    /// Get platform tag from this policy
    pub fn platform_tag(&self) -> PlatformTag {
        self.name.parse().expect("unknown platform tag")
    }

    /// Get policy by it's platform tag name
    pub fn from_name(name: &str) -> Option<Self> {
        let policies = if name.starts_with("musllinux") {
            &MUSLLINUX_POLICIES
        } else {
            &MANYLINUX_POLICIES
        };
        policies
            .iter()
            .find(|p| p.name == name || p.aliases.iter().any(|alias| alias == name))
            .cloned()
    }

    pub(crate) fn fixup_musl_libc_so_name(&mut self, target_arch: Arch) {
        // Fixup musl libc lib_whitelist
        if self.name.starts_with("musllinux") && self.lib_whitelist.remove("libc.so") {
            let new_soname = match target_arch {
                Arch::Aarch64 => "libc.musl-aarch64.so.1",
                Arch::Armv6L => "libc.musl-armhf.so.1",
                Arch::Armv7L => "libc.musl-armv7.so.1",
                Arch::Powerpc64Le => "libc.musl-ppc64le.so.1",
                Arch::Powerpc64 => "", // musllinux doesn't support ppc64
                Arch::X86 => "libc.musl-x86.so.1",
                Arch::X86_64 => "libc.musl-x86_64.so.1",
                Arch::S390X => "libc.musl-s390x.so.1",
                _ => "",
            };
            if !new_soname.is_empty() {
                self.lib_whitelist.insert(new_soname.to_string());
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::{Arch, Policy, MANYLINUX_POLICIES, MUSLLINUX_POLICIES};
    use pretty_assertions::assert_eq;

    #[test]
    fn test_load_policy() {
        let linux = Policy::from_name("linux").unwrap();
        assert!(linux.symbol_versions.is_empty());
        assert!(linux.lib_whitelist.is_empty());

        let manylinux2010 = Policy::from_name("manylinux2010").unwrap();
        assert!(manylinux2010.lib_whitelist.contains("libc.so.6"));
        let symbol_version = &manylinux2010.symbol_versions["x86_64"];
        assert_eq!(symbol_version["CXXABI"].len(), 4);
        let cxxabi = &symbol_version["CXXABI"];
        for version in &["1.3", "1.3.1", "1.3.2", "1.3.3"] {
            assert!(cxxabi.contains(*version));
        }
    }

    #[test]
    fn test_policy_manylinux_tag() {
        for policy in MANYLINUX_POLICIES.iter() {
            let _tag = policy.platform_tag();
        }
    }

    #[test]
    fn test_policy_musllinux_tag() {
        for policy in MUSLLINUX_POLICIES.iter() {
            let _tag = policy.platform_tag();
        }
    }

    #[test]
    fn test_policy_musllinux_fixup_libc_so_name() {
        let mut policy = Policy::from_name("musllinux_1_1").unwrap();
        policy.fixup_musl_libc_so_name(Arch::Aarch64);
        assert!(policy.lib_whitelist.contains("libc.musl-aarch64.so.1"));
    }
}
