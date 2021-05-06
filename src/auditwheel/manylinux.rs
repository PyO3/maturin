use crate::auditwheel::Policy;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Decides how to handle manylinux compliance
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq, Copy)]
pub enum Manylinux {
    /// Use the manylinux_x_y tag
    Manylinux {
        /// GLIBC version major
        x: u16,
        /// GLIBC version minor
        y: u16,
    },
    /// Use the native linux tag
    Off,
}

impl Manylinux {
    fn new(x: u16, y: u16) -> Self {
        Self::Manylinux { x, y }
    }

    /// `manylinux1` aka `manylinux_2_5`
    pub fn manylinux1() -> Self {
        Self::Manylinux { x: 2, y: 5 }
    }

    /// `manylinux2010` aka `manylinux_2_12`
    pub fn manylinux2010() -> Self {
        Self::Manylinux { x: 2, y: 12 }
    }

    /// `manylinux2014` aka `manylinux_2_17`
    pub fn manylinux2014() -> Self {
        Self::Manylinux { x: 2, y: 17 }
    }

    /// manylinux aliases
    pub fn aliases(&self) -> Vec<String> {
        match self {
            Manylinux::Manylinux { .. } => {
                if let Some(policy) = Policy::from_name(&self.to_string()) {
                    policy.aliases
                } else {
                    Vec::new()
                }
            }
            Manylinux::Off => Vec::new(),
        }
    }
}

impl fmt::Display for Manylinux {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Manylinux::Manylinux { x, y } => write!(f, "manylinux_{}_{}", x, y),
            Manylinux::Off => write!(f, "linux"),
        }
    }
}

impl FromStr for Manylinux {
    type Err = &'static str;

    fn from_str(value: &str) -> anyhow::Result<Self, Self::Err> {
        match value {
            "off" | "linux" => Ok(Manylinux::Off),
            "auto" | "1" | "manylinux1" => Ok(Manylinux::manylinux1()),
            "2010" | "manylinux2010" => Ok(Manylinux::manylinux2010()),
            "2014" | "manylinux2014" => Ok(Manylinux::manylinux2014()),
            _ => {
                let value = value.strip_prefix("manylinux_").unwrap_or(value);
                let mut parts = value.split('_');
                let x = parts
                    .next()
                    .and_then(|x| x.parse::<u16>().ok())
                    .ok_or("invalid manylinux option")?;
                let y = parts
                    .next()
                    .and_then(|y| y.parse::<u16>().ok())
                    .ok_or("invalid manylinux option")?;
                Ok(Manylinux::new(x, y))
            }
        }
    }
}
