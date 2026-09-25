use std::collections::BTreeSet;
use std::fmt;

use itertools::Itertools as _;

/// A PEP 425 wheel tag with optional compressed (dot-separated) components.
///
/// Compressed tags such as `py2.py3-none-any` expand to one fully qualified tag
/// per combination of components via [`WheelTag::expand`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WheelTag {
    python: BTreeSet<String>,
    abi: BTreeSet<String>,
    platform: BTreeSet<String>,
}

impl WheelTag {
    /// Create a wheel tag from python, ABI, and platform components.
    ///
    /// Each component is a sorted set of individual tags.
    pub fn new(
        python: impl Into<BTreeSet<String>>,
        abi: impl Into<BTreeSet<String>>,
        platform: impl Into<BTreeSet<String>>,
    ) -> Self {
        Self {
            python: python.into(),
            abi: abi.into(),
            platform: platform.into(),
        }
    }

    /// The Python tags (e.g. `cp312`, `pp311`, `py3`).
    pub fn python(&self) -> &BTreeSet<String> {
        &self.python
    }

    /// The ABI tags (e.g. `cp312`, `abi3`, `none`).
    pub fn abi(&self) -> &BTreeSet<String> {
        &self.abi
    }

    /// The platform tags (e.g. `manylinux_2_17_x86_64`, `any`).
    pub fn platform(&self) -> &BTreeSet<String> {
        &self.platform
    }

    /// Expand compressed components into fully qualified PEP 425 tags.
    pub fn expand(&self) -> impl Iterator<Item = String> + '_ {
        self.python
            .iter()
            .cartesian_product(&self.abi)
            .cartesian_product(&self.platform)
            .map(|((python, abi), platform)| format!("{python}-{abi}-{platform}"))
    }
}

impl fmt::Display for WheelTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}-{}",
            self.python.iter().format("."),
            self.abi.iter().format("."),
            self.platform.iter().format(".")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::WheelTag;

    #[test]
    fn display_renders_pep425_tag() {
        let tag = WheelTag::new(
            ["cp312".to_string()],
            ["cp312".to_string()],
            ["manylinux_2_17_x86_64".to_string()],
        );

        assert_eq!(tag.to_string(), "cp312-cp312-manylinux_2_17_x86_64");
    }

    #[test]
    fn display_sorts_compressed_tag_sets() {
        let tag = WheelTag::new(
            ["cp39".to_string(), "cp310".to_string()],
            ["abi3t".to_string(), "abi3".to_string()],
            [
                "manylinux_2_17_x86_64".to_string(),
                "manylinux2014_x86_64".to_string(),
            ],
        );
        assert_eq!(
            tag.to_string(),
            "cp310.cp39-abi3.abi3t-manylinux2014_x86_64.manylinux_2_17_x86_64"
        );

        let universal2 = WheelTag::new(
            ["py3".to_string()],
            ["none".to_string()],
            [
                "macosx_10_12_x86_64".to_string(),
                "macosx_11_0_arm64".to_string(),
                "macosx_10_12_universal2".to_string(),
            ],
        );
        assert_eq!(
            universal2.to_string(),
            "py3-none-macosx_10_12_universal2.macosx_10_12_x86_64.macosx_11_0_arm64"
        );
    }

    #[test]
    fn expand_compressed_tags() {
        let expanded = WheelTag::new(
            ["py2".to_string(), "py3".to_string()],
            ["none".to_string()],
            ["any".to_string()],
        )
        .expand()
        .collect::<Vec<_>>();

        assert_eq!(expanded, ["py2-none-any", "py3-none-any"]);
    }

    #[test]
    fn expand_compressed_platform_tags() {
        let expanded = WheelTag::new(
            ["cp37".to_string()],
            ["abi3".to_string()],
            [
                "manylinux_2_17_x86_64".to_string(),
                "manylinux2014_x86_64".to_string(),
            ],
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
            ["cp315".to_string()],
            ["abi3".to_string(), "abi3t".to_string()],
            ["manylinux_2_17_x86_64".to_string()],
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
}
