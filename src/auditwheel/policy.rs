use crate::auditwheel::Manylinux;
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::cmp::{Ordering, PartialOrd};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::{Display, Formatter};

/// The policies (allowed symbols) for the different manylinux tags, sorted from highest
/// priority to lowest
pub static POLICIES: Lazy<Vec<Policy>> = Lazy::new(|| {
    // https://github.com/pypa/auditwheel/blob/master/auditwheel/policy/policy.json
    let mut policies: Vec<Policy> = serde_json::from_slice(include_bytes!("policy.json"))
        .expect("invalid manylinux policy.json file");
    policies.sort_by_key(|policy| -policy.priority);
    policies
});

/// Manylinux policy
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Policy {
    /// manylinux platform tag name
    pub name: String,
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
        Policy::from_priority(0).unwrap()
    }
}

impl Display for Policy {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
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
        POLICIES.iter().filter(move |p| p.priority > self.priority)
    }

    /// Get manylinux platform tag from this policy
    pub fn manylinux_tag(&self) -> Manylinux {
        self.name.parse().expect("Manylinux variants is incomplete")
    }

    /// Get policy by it's manylinux platform tag name
    pub fn from_name(name: &str) -> Option<Self> {
        POLICIES.iter().find(|p| p.name == name).cloned()
    }

    /// Get policy by it's priority
    pub fn from_priority(priority: i64) -> Option<Self> {
        POLICIES.iter().find(|p| p.priority == priority).cloned()
    }
}

#[cfg(test)]
mod test {
    use super::{Policy, POLICIES};

    #[test]
    fn test_load_policy() {
        let linux = POLICIES.iter().find(|p| p.name == "linux").unwrap();
        assert!(linux.symbol_versions.is_empty());
        assert!(linux.lib_whitelist.is_empty());

        let manylinux2010 = POLICIES.iter().find(|p| p.name == "manylinux2010").unwrap();
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
        for policy in POLICIES.iter() {
            let _tag = policy.manylinux_tag();
        }
    }

    #[test]
    fn test_policy_from_name() {
        use crate::auditwheel::Manylinux;

        let tags = &[
            Manylinux::Manylinux1,
            Manylinux::Manylinux2010,
            Manylinux::Manylinux2014,
            Manylinux::Manylinux_2_24,
            Manylinux::Off,
        ];
        for manylinux in tags {
            let policy = Policy::from_name(&manylinux.to_string());
            assert!(policy.is_some());
        }
    }
}
