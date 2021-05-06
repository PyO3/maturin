mod audit;
mod manylinux;
mod policy;

pub use self::audit::*;
pub use manylinux::Manylinux;
pub use policy::{Policy, POLICIES};

/// auditwheel manylinux policy
#[derive(Debug, Clone)]
pub struct ManylinuxPolicy {
    /// User specified policy
    pub policy: Policy,
    /// Highest matching policy
    pub highest_policy: Option<Policy>,
}

impl Default for ManylinuxPolicy {
    fn default() -> Self {
        // defaults to linux
        ManylinuxPolicy {
            policy: Policy::default(),
            highest_policy: None,
        }
    }
}
