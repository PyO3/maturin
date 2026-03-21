use crate::pyproject_toml::{FeatureSpec, ToolMaturin};
use anyhow::Result;
use cargo_options::heading;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;

/// A Rust target triple or a virtual target triple.
#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
pub enum TargetTriple {
    /// The virtual `universal2-apple-darwin` target triple, build a fat binary of
    /// `aarch64-apple-darwin` and `x86_64-apple-darwin`.
    Universal2,
    /// Any target triple supported by Rust.
    ///
    /// It's not guaranteed that the value exists, it's passed verbatim to Cargo.
    Regular(String),
}

impl FromStr for TargetTriple {
    // TODO: Use the never type once stabilized
    type Err = String;

    fn from_str(triple: &str) -> std::result::Result<Self, Self::Err> {
        match triple {
            "universal2-apple-darwin" => Ok(TargetTriple::Universal2),
            triple => Ok(TargetTriple::Regular(triple.to_string())),
        }
    }
}

/// Cargo options for the build process
#[derive(Debug, Default, Serialize, Deserialize, clap::Parser, Clone, Eq, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
pub struct CargoOptions {
    /// Do not print cargo log messages
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Number of parallel jobs, defaults to # of CPUs
    #[arg(short = 'j', long, value_name = "N", help_heading = heading::COMPILATION_OPTIONS)]
    pub jobs: Option<usize>,

    /// Build artifacts with the specified Cargo profile
    #[arg(long, value_name = "PROFILE-NAME", help_heading = heading::COMPILATION_OPTIONS)]
    pub profile: Option<String>,

    /// Space or comma separated list of features to activate
    #[arg(
        short = 'F',
        long,
        action = clap::ArgAction::Append,
        help_heading = heading::FEATURE_SELECTION,
    )]
    pub features: Vec<String>,

    /// Activate all available features
    #[arg(long, help_heading = heading::FEATURE_SELECTION)]
    pub all_features: bool,

    /// Do not activate the `default` feature
    #[arg(long, help_heading = heading::FEATURE_SELECTION)]
    pub no_default_features: bool,

    /// Build for the target triple
    #[arg(
        long,
        value_name = "TRIPLE",
        env = "CARGO_BUILD_TARGET",
        help_heading = heading::COMPILATION_OPTIONS,
    )]
    pub target: Option<TargetTriple>,

    /// Directory for all generated artifacts
    #[arg(long, value_name = "DIRECTORY", help_heading = heading::COMPILATION_OPTIONS)]
    pub target_dir: Option<PathBuf>,

    /// Path to Cargo.toml
    #[arg(short = 'm', long, value_name = "PATH", help_heading = heading::MANIFEST_OPTIONS)]
    pub manifest_path: Option<PathBuf>,

    /// Ignore `rust-version` specification in packages
    #[arg(long)]
    pub ignore_rust_version: bool,

    /// Use verbose output (-vv very verbose/build.rs output)
    // Note that this duplicates the global option, but clap seems to be fine with that.
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Coloring: auto, always, never
    #[arg(long, value_name = "WHEN")]
    pub color: Option<String>,

    /// Require Cargo.lock and cache are up to date
    #[arg(long, help_heading = heading::MANIFEST_OPTIONS)]
    pub frozen: bool,

    /// Require Cargo.lock is up to date
    #[arg(long, help_heading = heading::MANIFEST_OPTIONS)]
    pub locked: bool,

    /// Run without accessing the network
    #[arg(long, help_heading = heading::MANIFEST_OPTIONS)]
    pub offline: bool,

    /// Override a configuration value (unstable)
    #[arg(long, value_name = "KEY=VALUE", action = clap::ArgAction::Append)]
    pub config: Vec<String>,

    /// Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details
    #[arg(short = 'Z', value_name = "FLAG", action = clap::ArgAction::Append)]
    pub unstable_flags: Vec<String>,

    /// Timing output formats (unstable) (comma separated): html, json
    #[arg(
        long,
        value_name = "FMTS",
        value_delimiter = ',',
        require_equals = true,
        help_heading = heading::COMPILATION_OPTIONS,
    )]
    pub timings: Option<Vec<String>>,

    /// Outputs a future incompatibility report at the end of the build (unstable)
    #[arg(long)]
    pub future_incompat_report: bool,

    /// Rustc flags
    #[arg(num_args = 0.., trailing_var_arg = true)]
    pub args: Vec<String>,
}

impl CargoOptions {
    /// Extract `cargo metadata` extra arguments from the cargo options.
    pub fn cargo_metadata_args(&self) -> Result<Vec<String>> {
        let mut args = vec![];
        if self.frozen {
            args.push("--frozen".to_string());
        }
        if self.locked {
            args.push("--locked".to_string());
        }
        if self.offline {
            args.push("--offline".to_string());
        }
        for feature in &self.features {
            args.push("--features".to_string());
            args.push(feature.clone());
        }
        if self.all_features {
            args.push("--all-features".to_string());
        }
        if self.no_default_features {
            args.push("--no-default-features".to_string());
        }
        if let Some(target) = &self.target {
            match target {
                TargetTriple::Universal2 => {
                    args.extend([
                        "--filter-platform".to_string(),
                        "aarch64-apple-darwin".to_string(),
                        "--filter-platform".to_string(),
                        "x86_64-apple-darwin".to_string(),
                    ]);
                }
                TargetTriple::Regular(target) => {
                    args.push("--filter-platform".to_string());
                    args.push(target.clone());
                }
            }
        }
        for opt in &self.unstable_flags {
            args.push("-Z".to_string());
            args.push(opt.clone());
        }
        Ok(args)
    }

    /// Convert the Cargo options into a Cargo invocation.
    pub fn into_rustc_options(self, target_triple: Option<String>) -> cargo_options::Rustc {
        cargo_options::Rustc {
            common: cargo_options::CommonOptions {
                quiet: self.quiet,
                jobs: self.jobs,
                profile: self.profile,
                features: self.features,
                all_features: self.all_features,
                no_default_features: self.no_default_features,
                target: if let Some(target) = target_triple {
                    vec![target]
                } else {
                    Vec::new()
                },
                target_dir: self.target_dir,
                verbose: self.verbose,
                color: self.color,
                frozen: self.frozen,
                locked: self.locked,
                offline: self.offline,
                config: self.config,
                unstable_flags: self.unstable_flags,
                timings: self.timings,
                ..Default::default()
            },
            manifest_path: self.manifest_path,
            ignore_rust_version: self.ignore_rust_version,
            future_incompat_report: self.future_incompat_report,
            args: self.args,
            ..Default::default()
        }
    }

    /// Merge options from pyproject.toml
    pub fn merge_with_pyproject_toml(
        &mut self,
        tool_maturin: ToolMaturin,
        editable_install: bool,
    ) -> Vec<&'static str> {
        let mut args_from_pyproject = Vec::new();

        if self.manifest_path.is_none() && tool_maturin.manifest_path.is_some() {
            self.manifest_path.clone_from(&tool_maturin.manifest_path);
            args_from_pyproject.push("manifest-path");
        }

        if self.profile.is_none() {
            // For `maturin` v1 compatibility, `editable-profile` falls back to `profile` if unset.
            // TODO: on `maturin` v2, consider defaulting to "dev" profile for editable installs,
            // and potentially remove this fallback behavior.
            let (tool_profile, source_variable) =
                if editable_install && tool_maturin.editable_profile.is_some() {
                    (tool_maturin.editable_profile.as_ref(), "editable-profile")
                } else {
                    (tool_maturin.profile.as_ref(), "profile")
                };
            if let Some(tool_profile) = tool_profile {
                self.profile = Some(tool_profile.clone());
                args_from_pyproject.push(source_variable);
            }
        }

        if let Some(feature_specs) = tool_maturin.features
            && self.features.is_empty()
        {
            let (plain, _conditional) = FeatureSpec::split(feature_specs);
            self.features = plain;
            args_from_pyproject.push("features");
        }

        if let Some(all_features) = tool_maturin.all_features
            && !self.all_features
        {
            self.all_features = all_features;
            args_from_pyproject.push("all-features");
        }

        if let Some(no_default_features) = tool_maturin.no_default_features
            && !self.no_default_features
        {
            self.no_default_features = no_default_features;
            args_from_pyproject.push("no-default-features");
        }

        if let Some(frozen) = tool_maturin.frozen
            && !self.frozen
        {
            self.frozen = frozen;
            args_from_pyproject.push("frozen");
        }

        if let Some(locked) = tool_maturin.locked
            && !self.locked
        {
            self.locked = locked;
            args_from_pyproject.push("locked");
        }

        if let Some(config) = tool_maturin.config
            && self.config.is_empty()
        {
            self.config = config;
            args_from_pyproject.push("config");
        }

        if let Some(unstable_flags) = tool_maturin.unstable_flags
            && self.unstable_flags.is_empty()
        {
            self.unstable_flags = unstable_flags;
            args_from_pyproject.push("unstable-flags");
        }

        args_from_pyproject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_old_extra_feature_args() {
        let cargo_extra_args = CargoOptions {
            no_default_features: true,
            features: vec!["a".to_string(), "c".to_string()],
            target: Some(TargetTriple::Regular(
                "x86_64-unknown-linux-musl".to_string(),
            )),
            ..Default::default()
        };
        let cargo_metadata_extra_args = cargo_extra_args.cargo_metadata_args().unwrap();
        assert_eq!(
            cargo_metadata_extra_args,
            vec![
                "--features",
                "a",
                "--features",
                "c",
                "--no-default-features",
                "--filter-platform",
                "x86_64-unknown-linux-musl",
            ]
        );
    }

    #[test]
    fn test_extract_cargo_metadata_args() {
        let args = CargoOptions {
            locked: true,
            features: vec!["my-feature".to_string(), "other-feature".to_string()],
            target: Some(TargetTriple::Regular(
                "x86_64-unknown-linux-musl".to_string(),
            )),
            unstable_flags: vec!["unstable-options".to_string()],
            ..Default::default()
        };

        let expected = vec![
            "--locked",
            "--features",
            "my-feature",
            "--features",
            "other-feature",
            "--filter-platform",
            "x86_64-unknown-linux-musl",
            "-Z",
            "unstable-options",
        ];

        assert_eq!(args.cargo_metadata_args().unwrap(), expected);
    }
}
