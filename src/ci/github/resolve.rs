use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};

use super::super::{GenerateCI, Platform, ResolvedCIConfig, ResolvedTarget};
use crate::BridgeModel;
use crate::pyproject_toml::{CIConfigOverrides, GitHubCIConfig, PlatformCIConfig, TargetCIConfig};

/// Resolve a field using the chain: per-target → platform-level → default.
fn resolve_optional(
    per_target: Option<&str>,
    platform_level: Option<&str>,
    default: Option<&str>,
) -> Option<String> {
    per_target
        .or(platform_level)
        .or(default)
        .map(ToOwned::to_owned)
}

fn validate_platform_config(platform_name: &str, config: &PlatformCIConfig) -> Result<()> {
    if config.targets.is_some() && config.target.is_some() {
        bail!(
            "[tool.maturin.generate-ci.github.{}]: `targets` and `[[target]]` are mutually exclusive",
            platform_name
        );
    }
    if matches!(config.targets.as_ref(), Some(targets) if targets.is_empty()) {
        bail!(
            "[tool.maturin.generate-ci.github.{}]: `targets` must not be empty",
            platform_name
        );
    }
    if matches!(config.target.as_ref(), Some(targets) if targets.is_empty()) {
        bail!(
            "[tool.maturin.generate-ci.github.{}]: `[[target]]` must not be empty",
            platform_name
        );
    }
    Ok(())
}

fn configured_platforms(
    github_config: &GitHubCIConfig,
    bridge_model: &BridgeModel,
) -> BTreeSet<Platform> {
    let mut platforms = BTreeSet::new();
    if github_config.linux.is_some() {
        platforms.insert(Platform::ManyLinux);
    }
    if github_config.musllinux.is_some() {
        platforms.insert(Platform::Musllinux);
    }
    if github_config.windows.is_some() {
        platforms.insert(Platform::Windows);
    }
    if github_config.macos.is_some() {
        platforms.insert(Platform::Macos);
    }
    if github_config.emscripten.is_some() && !bridge_model.is_bin() {
        platforms.insert(Platform::Emscripten);
    }
    if github_config.android.is_some() {
        platforms.insert(Platform::Android);
    }
    platforms
}

fn platform_config_for(
    github_config: Option<&GitHubCIConfig>,
    platform: Platform,
) -> Option<&PlatformCIConfig> {
    github_config.and_then(|gh| match platform {
        Platform::ManyLinux => gh.linux.as_ref(),
        Platform::Musllinux => gh.musllinux.as_ref(),
        Platform::Windows => gh.windows.as_ref(),
        Platform::Macos => gh.macos.as_ref(),
        Platform::Emscripten => gh.emscripten.as_ref(),
        Platform::Android => gh.android.as_ref(),
        Platform::All => None,
    })
}

fn arch_list(platform: Platform, platform_config: Option<&PlatformCIConfig>) -> Vec<String> {
    match platform_config {
        Some(config) if config.target.is_some() => config
            .target
            .as_ref()
            .unwrap()
            .iter()
            .map(|t| t.arch.clone())
            .collect(),
        Some(config) if config.targets.is_some() => config.targets.clone().unwrap(),
        _ => platform
            .default_targets()
            .iter()
            .map(|target| (*target).to_string())
            .collect(),
    }
}

fn find_target_config<'a>(
    platform_config: Option<&'a PlatformCIConfig>,
    arch: &str,
) -> Option<&'a TargetCIConfig> {
    platform_config.and_then(|config| {
        config
            .target
            .as_ref()
            .and_then(|targets| targets.iter().find(|target| target.arch == arch))
    })
}

/// Resolve targets for a given platform from pyproject config + hardcoded defaults.
pub(crate) fn resolve_platform_targets(
    platform: Platform,
    platform_config: Option<&PlatformCIConfig>,
    github_config: Option<&GitHubCIConfig>,
) -> Result<Vec<ResolvedTarget>> {
    let platform_name = platform.to_string();
    if let Some(config) = platform_config {
        validate_platform_config(&platform_name, config)?;
    }

    let mut resolved = Vec::new();
    for arch in arch_list(platform, platform_config) {
        let per_target = find_target_config(platform_config, &arch);
        let python_arch = platform.default_python_arch(&arch);

        let pt = |field: fn(&CIConfigOverrides) -> &Option<String>| {
            per_target.and_then(|target| field(&target.overrides).as_deref())
        };
        let pl = |field: fn(&CIConfigOverrides) -> &Option<String>| {
            platform_config.and_then(|config| field(&config.overrides).as_deref())
        };

        let runner = resolve_optional(
            pt(|o| &o.runner),
            pl(|o| &o.runner),
            Some(platform.default_runner(&arch)),
        )
        .unwrap();

        resolved.push(ResolvedTarget {
            runner,
            target: arch,
            python_arch,
            manylinux: resolve_optional(
                pt(|o| &o.manylinux),
                pl(|o| &o.manylinux),
                platform.default_manylinux(),
            ),
            container: resolve_optional(pt(|o| &o.container), pl(|o| &o.container), None),
            docker_options: resolve_optional(
                pt(|o| &o.docker_options),
                pl(|o| &o.docker_options),
                None,
            ),
            rust_toolchain: resolve_optional(
                pt(|o| &o.rust_toolchain),
                pl(|o| &o.rust_toolchain),
                platform.default_rust_toolchain(),
            ),
            rustup_components: resolve_optional(
                pt(|o| &o.rustup_components),
                pl(|o| &o.rustup_components),
                None,
            ),
            before_script_linux: resolve_optional(
                pt(|o| &o.before_script_linux),
                pl(|o| &o.before_script_linux),
                None,
            ),
            extra_args: resolve_optional(
                pt(|o| &o.args),
                pl(|o| &o.args),
                github_config.and_then(|config| config.args.as_deref()),
            ),
        });
    }

    Ok(resolved)
}

/// Build a ResolvedCIConfig from CLI args + pyproject GitHubCIConfig.
pub(crate) fn resolve_config(
    cli: &GenerateCI,
    github_config: Option<&GitHubCIConfig>,
    bridge_model: &BridgeModel,
) -> Result<ResolvedCIConfig> {
    let pytest = cli.pytest
        || github_config
            .and_then(|config| config.pytest)
            .unwrap_or(false);
    let zig = cli.zig || github_config.and_then(|config| config.zig).unwrap_or(false);
    let skip_attestation = cli.skip_attestation
        || github_config
            .and_then(|config| config.skip_attestation)
            .unwrap_or(false);

    let platforms: BTreeSet<Platform> = if !cli.platforms.is_empty() {
        cli.platforms
            .iter()
            .flat_map(|platform| {
                if matches!(platform, Platform::All) {
                    if bridge_model.is_bin() {
                        Platform::defaults()
                    } else {
                        Platform::all()
                    }
                } else {
                    std::slice::from_ref(platform)
                }
            })
            .filter(|platform| !bridge_model.is_bin() || !matches!(platform, Platform::Emscripten))
            .copied()
            .collect()
    } else if let Some(config) = github_config {
        let platforms = configured_platforms(config, bridge_model);
        if platforms.is_empty() {
            Platform::defaults().iter().copied().collect()
        } else {
            platforms
        }
    } else {
        Platform::defaults().iter().copied().collect()
    };

    let mut platform_targets = BTreeMap::new();
    for &platform in &platforms {
        let platform_config = platform_config_for(github_config, platform);
        let targets = resolve_platform_targets(platform, platform_config, github_config)?;
        platform_targets.insert(platform, targets);
    }

    Ok(ResolvedCIConfig {
        pytest,
        zig,
        skip_attestation,
        platform_targets,
    })
}
