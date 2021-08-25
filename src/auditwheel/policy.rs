use crate::auditwheel::PlatformTag;
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
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Policy {
    /// manylinux platform tag name
    pub name: String,
    /// manylinux platform tag aliases
    pub aliases: Vec<String>,
    /// policy priority. Tags supporting more platforms have higher priority
    pub priority: i64,
    /// platform architecture to symbol versions map
    #[serde(rename = "symbol_versions")]
    pub symbol_versions: HashMap<String, HashMap<String, HashSet<String>>>,
    #[serde(rename = "lib_whitelist")]
    pub lib_whitelist: HashSet<String>,
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
                "{}(aka {})",
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
    /// Get highest priority policy than self
    pub fn higher_priority_policies(&self) -> impl Iterator<Item = &Policy> {
        let policies = if self.name.starts_with("musllinux") {
            &MUSLLINUX_POLICIES
        } else {
            &MANYLINUX_POLICIES
        };
        policies.iter().filter(move |p| p.priority > self.priority)
    }

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
}

#[cfg(test)]
mod test {
    use super::{Policy, MANYLINUX_POLICIES, MUSLLINUX_POLICIES};

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
}
