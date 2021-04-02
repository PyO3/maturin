use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Decides how to handle manylinux compliance
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq, Copy)]
pub enum Manylinux {
    /// Use the manylinux1 tag
    Manylinux1,
    /// Use the manylinux2010 tag
    Manylinux2010,
    /// Use the manylinux2014 tag
    Manylinux2014,
    /// Use the manylinux_2_24 tag
    #[allow(non_camel_case_types)]
    Manylinux_2_24,
    /// Use the native linux tag
    Off,
}

impl fmt::Display for Manylinux {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Manylinux::Manylinux1 => write!(f, "manylinux1"),
            Manylinux::Manylinux2010 => write!(f, "manylinux2010"),
            Manylinux::Manylinux2014 => write!(f, "manylinux2014"),
            Manylinux::Manylinux_2_24 => write!(f, "manylinux_2_24"),
            Manylinux::Off => write!(f, "linux"),
        }
    }
}

impl FromStr for Manylinux {
    type Err = &'static str;

    fn from_str(value: &str) -> anyhow::Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Manylinux::Manylinux1),
            "1" | "manylinux1" => Ok(Manylinux::Manylinux1),
            "2010" | "manylinux2010" => Ok(Manylinux::Manylinux2010),
            "2014" | "manylinux2014" => Ok(Manylinux::Manylinux2014),
            "2_24" | "manylinux_2_24" => Ok(Manylinux::Manylinux_2_24),
            "off" | "linux" => Ok(Manylinux::Off),
            _ => Err("Invalid value for the manylinux option"),
        }
    }
}
