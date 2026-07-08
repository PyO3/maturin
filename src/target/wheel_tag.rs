use std::fmt;
use std::str::FromStr;

use anyhow::bail;
use itertools::Itertools as _;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WheelTag {
    python: String,
    abi: String,
    platform: String,
}

impl WheelTag {
    pub(crate) fn new(
        python: impl Into<String>,
        abi: impl Into<String>,
        platform: impl Into<String>,
    ) -> Self {
        Self {
            python: python.into(),
            abi: abi.into(),
            platform: platform.into(),
        }
    }

    pub(crate) fn python(&self) -> &str {
        &self.python
    }

    pub(crate) fn expand(&self) -> impl Iterator<Item = String> + '_ {
        [&self.python, &self.abi, &self.platform]
            .into_iter()
            .map(|component| component.split('.'))
            .multi_cartesian_product()
            .map(|components| components.join("-"))
    }
}

impl fmt::Display for WheelTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{}", self.python, self.abi, self.platform)
    }
}

impl FromStr for WheelTag {
    type Err = anyhow::Error;

    fn from_str(tag: &str) -> std::result::Result<Self, Self::Err> {
        let mut components = tag.split('-');
        let Some(python) = components.next() else {
            bail!("wheel tag must contain a python tag: {tag}");
        };
        let Some(abi) = components.next() else {
            bail!("wheel tag must contain an ABI tag: {tag}");
        };
        let Some(platform) = components.next() else {
            bail!("wheel tag must contain a platform tag: {tag}");
        };
        if components.next().is_some() {
            bail!("wheel tag must have exactly three components: {tag}");
        }

        Ok(Self::new(python, abi, platform))
    }
}

#[cfg(test)]
mod tests {
    use super::WheelTag;

    #[test]
    fn display_renders_pep425_tag() {
        let tag = WheelTag::new("cp312", "cp312", "manylinux_2_17_x86_64");

        assert_eq!(tag.to_string(), "cp312-cp312-manylinux_2_17_x86_64");
    }

    #[test]
    fn expand_compressed_tags() {
        let expanded = WheelTag::new("py2.py3", "none", "any")
            .expand()
            .collect::<Vec<_>>();

        assert_eq!(expanded, ["py2-none-any", "py3-none-any"]);
    }

    #[test]
    fn expand_compressed_platform_tags() {
        let expanded = WheelTag::new("cp37", "abi3", "manylinux_2_17_x86_64.manylinux2014_x86_64")
            .expand()
            .collect::<Vec<_>>();

        assert_eq!(
            expanded,
            [
                "cp37-abi3-manylinux_2_17_x86_64",
                "cp37-abi3-manylinux2014_x86_64"
            ]
        );
    }

    #[test]
    fn expand_abi3t_to_abi3_and_abi3t() {
        let expanded = WheelTag::new("cp315", "abi3.abi3t", "manylinux_2_17_x86_64")
            .expand()
            .collect::<Vec<_>>();

        assert_eq!(
            expanded,
            [
                "cp315-abi3-manylinux_2_17_x86_64",
                "cp315-abi3t-manylinux_2_17_x86_64"
            ]
        );
    }

    #[test]
    fn parses_existing_string_boundary() {
        let tag = "py3-none-any".parse::<WheelTag>().unwrap();

        assert_eq!(tag, WheelTag::new("py3", "none", "any"));
    }
}
