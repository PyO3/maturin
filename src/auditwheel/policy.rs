use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

pub static POLICIES: Lazy<Vec<Policy>> = Lazy::new(|| {
    // https://github.com/pypa/auditwheel/blob/master/auditwheel/policy/policy.json
    serde_json::from_slice(include_bytes!("policy.json"))
        .expect("invalid manylinux policy.json file")
});

/// Manylinux policy
#[derive(Default, Debug, Clone, PartialEq, Deserialize)]
pub struct Policy {
    /// manylinux platform tag name
    pub name: String,
    /// policy priority
    pub priority: i64,
    /// platform architecture to `SymbolVersion` map
    #[serde(rename = "symbol_versions")]
    pub symbol_versions: HashMap<String, SymbolVersion>,
    #[serde(rename = "lib_whitelist")]
    pub lib_whitelist: HashSet<String>,
}

/// Manylinux symbol version policy
#[derive(Default, Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub struct SymbolVersion {
    pub cxxabi: Vec<String>,
    pub gcc: Vec<String>,
    pub glibc: Vec<String>,
    pub glibcxx: Vec<String>,
}

#[cfg(test)]
mod test {
    use super::POLICIES;

    #[test]
    fn test_load_policy() {
        let linux = POLICIES.iter().find(|p| p.name == "linux").unwrap();
        assert!(linux.symbol_versions.is_empty());
        assert!(linux.lib_whitelist.is_empty());

        let manylinux2010 = POLICIES.iter().find(|p| p.name == "manylinux2010").unwrap();
        assert!(manylinux2010.lib_whitelist.contains("libc.so.6"));
        let symbol_version = &manylinux2010.symbol_versions["x86_64"];
        assert_eq!(symbol_version.cxxabi, &["1.3", "1.3.1", "1.3.2", "1.3.3"]);
    }
}
