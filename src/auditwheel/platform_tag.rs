use crate::auditwheel::Policy;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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

    /// Is it supported by Rust compiler and manylinux project
    pub fn is_supported(&self) -> bool {
        match self {
            PlatformTag::Manylinux { major, minor } => (*major, *minor) >= (2, 17),
            PlatformTag::Musllinux { .. } => true,
            PlatformTag::Linux => true,
        }
    }
}

impl fmt::Display for PlatformTag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            PlatformTag::Manylinux { major, minor } => write!(f, "manylinux_{major}_{minor}"),
            PlatformTag::Musllinux { major, minor } => write!(f, "musllinux_{major}_{minor}"),
            PlatformTag::Linux => write!(f, "linux"),
        }
    }
}

impl FromStr for PlatformTag {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.to_ascii_lowercase();
        match value.as_str() {
            "off" | "linux" => Ok(PlatformTag::Linux),
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

/// Parsed `--compatibility` / `[tool.maturin] compatibility` value.
///
/// `pypi` is not a real platform tag. It is normalized by `BuildContextBuilder`
/// into a PyPI-validation flag and never stored on `PythonContext.platform_tag`.
///
/// Real platform tags are stored as [`CompatibilityTag::Platform`] so new
/// [`PlatformTag`] variants only need to be added in one place.
#[derive(Debug, Clone, Eq, PartialEq, Copy, Ord, PartialOrd)]
pub enum CompatibilityTag {
    /// A real platform tag (manylinux, musllinux, or linux).
    Platform(PlatformTag),
    /// Ensure that a PyPI-compatible tag is used, error if the target is not supported by PyPI.
    Pypi,
}

impl CompatibilityTag {
    /// Returns `None` for the `pypi` pseudo-option.
    pub fn into_platform_tag(self) -> Option<PlatformTag> {
        match self {
            CompatibilityTag::Platform(tag) => Some(tag),
            CompatibilityTag::Pypi => None,
        }
    }

    /// Is this the PyPI compatibility option
    pub fn is_pypi(&self) -> bool {
        matches!(self, CompatibilityTag::Pypi)
    }
}

impl From<PlatformTag> for CompatibilityTag {
    fn from(value: PlatformTag) -> Self {
        CompatibilityTag::Platform(value)
    }
}

impl fmt::Display for CompatibilityTag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            CompatibilityTag::Platform(tag) => write!(f, "{tag}"),
            CompatibilityTag::Pypi => write!(f, "pypi"),
        }
    }
}

impl FromStr for CompatibilityTag {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("pypi") {
            Ok(CompatibilityTag::Pypi)
        } else {
            PlatformTag::from_str(value).map(CompatibilityTag::Platform)
        }
    }
}

/// Serialize as the same string form used by CLI / TOML parsing.
impl Serialize for CompatibilityTag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CompatibilityTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FromStr::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for CompatibilityTag {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CompatibilityTag".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        "maturin::CompatibilityTag".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // TOML / CLI accept string values (e.g. "manylinux_2_17", "pypi"), not
        // externally-tagged objects. Document the string form so the schema matches
        // serde Deserialize and Serialize.
        schemars::json_schema!({
            "description": "Parsed `--compatibility` / `[tool.maturin] compatibility` value.\n\nAccepts platform tags such as `linux`, `manylinux2014`, `manylinux_2_17`, `musllinux_1_2`, or the `pypi` pseudo-option (PyPI filename validation; not a real platform tag).",
            "type": "string"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CompatibilityTag, PlatformTag};
    use std::str::FromStr;

    #[test]
    fn compatibility_tag_serde_round_trips_as_string() {
        for value in [
            CompatibilityTag::Pypi,
            CompatibilityTag::from(PlatformTag::Linux),
            CompatibilityTag::from(PlatformTag::manylinux2014()),
            CompatibilityTag::from(PlatformTag::Musllinux { major: 1, minor: 2 }),
        ] {
            let json = serde_json::to_string(&value).unwrap();
            // String form, not an externally-tagged object.
            assert!(
                json.starts_with('"') && json.ends_with('"'),
                "expected string JSON, got {json}"
            );
            let parsed: CompatibilityTag = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, value);
            assert_eq!(
                CompatibilityTag::from_str(value.to_string().as_str()),
                Ok(value)
            );
        }
    }

    #[test]
    fn into_platform_tag_strips_pypi() {
        assert_eq!(CompatibilityTag::Pypi.into_platform_tag(), None);
        assert_eq!(
            CompatibilityTag::from(PlatformTag::Linux).into_platform_tag(),
            Some(PlatformTag::Linux)
        );
    }
}
