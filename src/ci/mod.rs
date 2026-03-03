/// GitHub Actions CI generation
pub mod github;

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{ArgAction, Parser, ValueEnum};
use fs_err as fs;

use crate::CargoOptions;
use crate::bridge::find_bridge;
use crate::project_layout::ProjectResolver;

/// CI providers
#[derive(Debug, Clone, Copy, ValueEnum)]
#[clap(rename_all = "lower")]
pub enum Provider {
    /// GitHub
    GitHub,
}

/// Platform
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
#[clap(rename_all = "lower")]
pub enum Platform {
    /// All
    All,
    /// Manylinux
    #[clap(alias = "linux")]
    ManyLinux,
    /// Musllinux
    Musllinux,
    /// Windows
    Windows,
    /// macOS
    Macos,
    /// Emscripten
    Emscripten,
    /// Android
    Android,
}

impl Platform {
    pub(crate) fn defaults() -> Vec<Self> {
        vec![
            Platform::ManyLinux,
            Platform::Musllinux,
            Platform::Windows,
            Platform::Macos,
        ]
    }

    pub(crate) fn all() -> Vec<Self> {
        vec![
            Platform::ManyLinux,
            Platform::Musllinux,
            Platform::Windows,
            Platform::Macos,
            Platform::Emscripten,
            Platform::Android,
        ]
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Platform::All => write!(f, "all"),
            Platform::ManyLinux => write!(f, "linux"),
            Platform::Musllinux => write!(f, "musllinux"),
            Platform::Windows => write!(f, "windows"),
            Platform::Macos => write!(f, "macos"),
            Platform::Emscripten => write!(f, "emscripten"),
            Platform::Android => write!(f, "android"),
        }
    }
}

/// A fully-resolved target for CI generation
#[derive(Debug, Clone)]
pub(crate) struct ResolvedTarget {
    pub runner: String,
    pub target: String,
    pub python_arch: Option<String>,
    pub manylinux: Option<String>,
    pub container: Option<String>,
    pub docker_options: Option<String>,
    pub rust_toolchain: Option<String>,
    pub rustup_components: Option<String>,
    pub before_script_linux: Option<String>,
    pub extra_args: Option<String>,
}

/// Resolved CI configuration after merging CLI args with pyproject.toml
#[derive(Debug)]
pub(crate) struct ResolvedCIConfig {
    pub pytest: bool,
    pub zig: bool,
    pub skip_attestation: bool,
    pub platforms: BTreeSet<Platform>,
    pub platform_targets: std::collections::BTreeMap<Platform, Vec<ResolvedTarget>>,
}

/// Generate CI configuration
#[derive(Debug, Parser)]
pub struct GenerateCI {
    /// CI provider
    #[arg(value_enum, value_name = "CI")]
    pub ci: Provider,
    /// Path to Cargo.toml
    #[arg(short = 'm', long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,
    /// Output path
    #[arg(short = 'o', long, value_name = "PATH", default_value = "-")]
    pub output: PathBuf,
    /// Platform support [deprecated: use [tool.maturin.generate-ci.github] in pyproject.toml]
    #[arg(
        id = "platform",
        long,
        action = ArgAction::Append,
        num_args = 1..,
    )]
    pub platforms: Vec<Platform>,
    /// Enable pytest [deprecated: use [tool.maturin.generate-ci.github] in pyproject.toml]
    #[arg(long)]
    pub pytest: bool,
    /// Use zig to do cross compilation [deprecated: use [tool.maturin.generate-ci.github] in pyproject.toml]
    #[arg(long)]
    pub zig: bool,
    /// Skip artifact attestation [deprecated: use [tool.maturin.generate-ci.github] in pyproject.toml]
    #[arg(long)]
    pub skip_attestation: bool,
}

impl Default for GenerateCI {
    fn default() -> Self {
        Self {
            ci: Provider::GitHub,
            manifest_path: None,
            output: PathBuf::from("-"),
            platforms: Vec::new(),
            pytest: false,
            zig: false,
            skip_attestation: false,
        }
    }
}

impl GenerateCI {
    /// Execute this command
    pub fn execute(&self) -> Result<()> {
        let conf = self.generate()?;
        self.print(&conf)
    }

    /// Generate CI configuration
    pub fn generate(&self) -> Result<String> {
        // Emit deprecation warnings for CLI options
        self.warn_deprecated_cli_options();

        let cargo_options = CargoOptions {
            manifest_path: self.manifest_path.clone(),
            ..Default::default()
        };
        let ProjectResolver {
            cargo_metadata,
            pyproject_toml,
            project_layout,
            ..
        } = ProjectResolver::resolve(self.manifest_path.clone(), cargo_options, false, None)?;
        let pyproject = pyproject_toml.as_ref();
        let bridge = find_bridge(
            &cargo_metadata,
            pyproject.and_then(|x| x.bindings()),
            pyproject,
        )?;
        let project_name = pyproject
            .and_then(|project| project.project_name())
            .unwrap_or(&project_layout.extension_name);
        let sdist = pyproject_toml.is_some();

        // Read pyproject CI config
        let github_config = pyproject
            .and_then(|p| p.generate_ci())
            .and_then(|ci| ci.github.as_ref());

        match self.ci {
            Provider::GitHub => {
                let resolved = github::resolve_config(self, github_config, &bridge)?;
                github::generate_github(self, &resolved, project_name, &bridge, sdist)
            }
        }
    }

    fn warn_deprecated_cli_options(&self) {
        let hint = "Use [tool.maturin.generate-ci.github] in pyproject.toml instead.";
        if !self.platforms.is_empty() {
            eprintln!("⚠️  Warning: --platform is deprecated for `maturin generate-ci`. {hint}");
        }
        if self.pytest {
            eprintln!("⚠️  Warning: --pytest is deprecated for `maturin generate-ci`. {hint}");
        }
        if self.zig {
            eprintln!("⚠️  Warning: --zig is deprecated for `maturin generate-ci`. {hint}");
        }
        if self.skip_attestation {
            eprintln!(
                "⚠️  Warning: --skip-attestation is deprecated for `maturin generate-ci`. {hint}"
            );
        }
    }

    fn print(&self, conf: &str) -> Result<()> {
        if self.output == Path::new("-") {
            print!("{conf}");
        } else {
            fs::write(&self.output, conf)?;
        }
        Ok(())
    }
}
