use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use anyhow::bail;
use itertools::Itertools as _;

/// A PEP 425 wheel tag with optional compressed (dot-separated) components.
///
/// Compressed tags such as `py2.py3-none-any` expand to one fully qualified tag
/// per combination of components via [`WheelTag::expand`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WheelTag {
    python: String,
    abi: String,
    platform: BTreeSet<String>,
}

impl WheelTag {
    /// Create a wheel tag from python, ABI, and platform components.
    ///
    /// Python and ABI components may be compressed (dot-separated) lists, e.g.
    /// `py2.py3` or `abi3.abi3t`. Platform tags are passed as a sorted set.
    pub fn new(
        python: impl Into<String>,
        abi: impl Into<String>,
        platform: BTreeSet<String>,
    ) -> Self {
        Self {
            python: sort_compressed_tags(python.into()),
            abi: sort_compressed_tags(abi.into()),
            platform,
        }
    }

    /// The python tag component (e.g. `cp312`, `pp311`, `py3`).
    pub fn python(&self) -> &str {
        &self.python
    }

    /// The ABI tag component (e.g. `cp312`, `abi3`, `none`).
    pub fn abi(&self) -> &str {
        &self.abi
    }

    /// The platform tags (e.g. `manylinux_2_17_x86_64`, `any`).
    pub fn platform(&self) -> &BTreeSet<String> {
        &self.platform
    }

    /// Expand compressed components into fully qualified PEP 425 tags.
    pub fn expand(&self) -> impl Iterator<Item = String> + '_ {
        self.python
            .split('.')
            .cartesian_product(self.abi.split('.'))
            .cartesian_product(&self.platform)
            .map(|((python, abi), platform)| format!("{python}-{abi}-{platform}"))
    }
}

fn sort_compressed_tags(tags: String) -> String {
    if tags.contains('.') {
        tags.split('.').sorted_unstable().join(".")
    } else {
        tags
    }
}

impl fmt::Display for WheelTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}-{}",
            self.python,
            self.abi,
            self.platform.iter().format(".")
        )
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

        Ok(Self::new(
            python,
            abi,
            platform.split('.').map(str::to_string).collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::WheelTag;

    #[test]
    fn display_renders_pep425_tag() {
        let tag = WheelTag::new(
            "cp312",
            "cp312",
            ["manylinux_2_17_x86_64".to_string()].into(),
        );

        assert_eq!(tag.to_string(), "cp312-cp312-manylinux_2_17_x86_64");
    }

    #[test]
    fn display_sorts_compressed_tag_sets() {
        let tag = WheelTag::new(
            "cp39.cp310",
            "abi3t.abi3",
            [
                "manylinux_2_17_x86_64".to_string(),
                "manylinux2014_x86_64".to_string(),
            ]
            .into(),
        );
        assert_eq!(
            tag.to_string(),
            "cp310.cp39-abi3.abi3t-manylinux2014_x86_64.manylinux_2_17_x86_64"
        );

        let universal2 = WheelTag::new(
            "py3",
            "none",
            [
                "macosx_10_12_x86_64".to_string(),
                "macosx_11_0_arm64".to_string(),
                "macosx_10_12_universal2".to_string(),
            ]
            .into(),
        );
        assert_eq!(
            universal2.to_string(),
            "py3-none-macosx_10_12_universal2.macosx_10_12_x86_64.macosx_11_0_arm64"
        );
    }

    #[test]
    fn expand_compressed_tags() {
        let expanded = WheelTag::new("py2.py3", "none", ["any".to_string()].into())
            .expand()
            .collect::<Vec<_>>();

        assert_eq!(expanded, ["py2-none-any", "py3-none-any"]);
    }

    #[test]
    fn expand_compressed_platform_tags() {
        let expanded = WheelTag::new(
            "cp37",
            "abi3",
            [
                "manylinux_2_17_x86_64".to_string(),
                "manylinux2014_x86_64".to_string(),
            ]
            .into(),
        )
        .expand()
        .collect::<Vec<_>>();

        assert_eq!(
            expanded,
            [
                "cp37-abi3-manylinux2014_x86_64",
                "cp37-abi3-manylinux_2_17_x86_64"
            ]
        );
    }

    #[test]
    fn expand_abi3t_to_abi3_and_abi3t() {
        let expanded = WheelTag::new(
            "cp315",
            "abi3.abi3t",
            ["manylinux_2_17_x86_64".to_string()].into(),
        )
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

        assert_eq!(
            tag,
            WheelTag::new("py3", "none", ["any".to_string()].into())
        );
    }

    #[test]
    fn display_round_trips_through_from_str() {
        let original = WheelTag::new(
            "cp37",
            "abi3",
            [
                "manylinux_2_17_x86_64".to_string(),
                "manylinux2014_x86_64".to_string(),
            ]
            .into(),
        );
        let parsed = original.to_string().parse::<WheelTag>().unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn from_str_rejects_too_few_components() {
        let err = "cp37-abi3".parse::<WheelTag>().unwrap_err();
        assert!(
            err.to_string().contains("platform tag"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn from_str_rejects_too_many_components() {
        let err = "a-b-c-d".parse::<WheelTag>().unwrap_err();
        assert!(
            err.to_string().contains("exactly three components"),
            "unexpected error: {err}"
        );
    }
}
