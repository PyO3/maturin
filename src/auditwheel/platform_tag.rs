use crate::auditwheel::Policy;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;

/// Decides how to handle manylinux and musllinux compliance
#[derive(Serialize, Debug, Clone, Eq, PartialEq, Copy, Ord, PartialOrd)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum PlatformTag {
    /// Use the `manylinux_<major>_<minor>` tag
    Manylinux {
        /// GLIBC version major
        major: u16,
        /// GLIBC version minor
        minor: u16,
    },
    /// Use the `musllinux_<major>_<minor>` tag
    Musllinux {
        /// musl libc version major
        major: u16,
        /// musl libc version minor
        minor: u16,
    },
    /// Use the native linux tag
    Linux,
    /// Ensure that a PyPI-compatible tag is used, error if the target is not supported by PyPI.
    Pypi,
}

impl PlatformTag {
    /// `manylinux1` aka `manylinux_2_5`
    pub fn manylinux1() -> Self {
        Self::Manylinux { major: 2, minor: 5 }
    }

    /// `manylinux2010` aka `manylinux_2_12`
    pub fn manylinux2010() -> Self {
        Self::Manylinux {
            major: 2,
            minor: 12,
        }
    }

    /// `manylinux2014` aka `manylinux_2_17`
    pub fn manylinux2014() -> Self {
        Self::Manylinux {
            major: 2,
            minor: 17,
        }
    }

    /// manylinux aliases, namely `manylinux1`, `manylinux2010` and `manylinux2014`.
    pub fn aliases(&self) -> Vec<String> {
        Policy::from_tag(self)
            .map(|policy| policy.aliases)
            .unwrap_or_default()
    }

    /// Is this a portable linux platform tag
    ///
    /// Only manylinux and musllinux are portable
    pub fn is_portable(&self) -> bool {
        !matches!(self, PlatformTag::Linux)
    }

    /// Is this a manylinux platform tag
    pub fn is_manylinux(&self) -> bool {
        matches!(self, PlatformTag::Manylinux { .. })
    }

    /// Is this a musllinux platform tag
    pub fn is_musllinux(&self) -> bool {
        matches!(self, PlatformTag::Musllinux { .. })
    }

    /// Is this the PyPI compatibility option
    pub fn is_pypi(&self) -> bool {
        matches!(self, PlatformTag::Pypi)
    }

    /// Is it supported by Rust compiler and manylinux project
    pub fn is_supported(&self) -> bool {
        match self {
            PlatformTag::Manylinux { major, minor } => (*major, *minor) >= (2, 17),
            PlatformTag::Musllinux { .. } => true,
            PlatformTag::Linux => true,
            PlatformTag::Pypi => true,
        }
    }
}

impl fmt::Display for PlatformTag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            PlatformTag::Manylinux { major, minor } => write!(f, "manylinux_{major}_{minor}"),
            PlatformTag::Musllinux { major, minor } => write!(f, "musllinux_{major}_{minor}"),
            PlatformTag::Linux => write!(f, "linux"),
            PlatformTag::Pypi => write!(f, "pypi"),
        }
    }
}

impl FromStr for PlatformTag {
    type Err = &'static str;

    fn from_str(value: &str) -> anyhow::Result<Self, Self::Err> {
        let value = value.to_ascii_lowercase();
        match value.as_str() {
            "off" | "linux" => Ok(PlatformTag::Linux),
            "pypi" => Ok(PlatformTag::Pypi),
            "1" | "manylinux1" => Ok(PlatformTag::manylinux1()),
            "2010" | "manylinux2010" => Ok(PlatformTag::manylinux2010()),
            "2014" | "manylinux2014" => Ok(PlatformTag::manylinux2014()),
            _ => {
                if let Some(value) = value.strip_prefix("musllinux_") {
                    let mut parts = value.split('_');
                    let major = parts
                        .next()
                        .and_then(|major| major.parse::<u16>().ok())
                        .ok_or("invalid musllinux option")?;
                    let minor = parts
                        .next()
                        .and_then(|minor| minor.parse::<u16>().ok())
                        .ok_or("invalid musllinux option")?;
                    Ok(PlatformTag::Musllinux { major, minor })
                } else {
                    let value = value.strip_prefix("manylinux_").unwrap_or(&value);
                    let mut parts = value.split('_');
                    let major = parts
                        .next()
                        .and_then(|major| major.parse::<u16>().ok())
                        .ok_or("invalid manylinux option")?;
                    let minor = parts
                        .next()
                        .and_then(|minor| minor.parse::<u16>().ok())
                        .ok_or("invalid manylinux option")?;
                    Ok(PlatformTag::Manylinux { major, minor })
                }
            }
        }
    }
}

impl<'de> Deserialize<'de> for PlatformTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FromStr::from_str(&s).map_err(serde::de::Error::custom)
    }
}
