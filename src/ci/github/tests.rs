use expect_test::expect_file;
use semver::Version;

use super::{generate_github, generate_github_from_cli, resolve_config};
use crate::ci::{GenerateCI, Platform};
use crate::pyproject_toml::{CIConfigOverrides, GitHubCIConfig, PlatformCIConfig, TargetCIConfig};
use crate::{Abi3Version, BridgeModel, PyO3, bridge::PyO3Crate};

const PROJECT_NAME: &str = "example";

/// Strip the first 5 header lines and return the rest with a trailing newline
/// so the snapshot files are compatible with the `fix end of files` pre-commit hook.
fn strip_header(output: &str) -> String {
    let mut stripped = output.lines().skip(5).collect::<Vec<_>>().join("\n");
    stripped.push('\n');
    stripped
}

fn assert_snapshot(output: &str, snapshot: &str) {
    let conf = strip_header(output);
    match snapshot {
        "github_default.yml" => expect_file!["__snapshot__/github_default.yml"].assert_eq(&conf),
        "github_abi3.yml" => expect_file!["__snapshot__/github_abi3.yml"].assert_eq(&conf),
        "github_no_attestations.yml" => {
            expect_file!["__snapshot__/github_no_attestations.yml"].assert_eq(&conf)
        }
        "github_zig_pytest.yml" => {
            expect_file!["__snapshot__/github_zig_pytest.yml"].assert_eq(&conf)
        }
        "github_bin_no_binding.yml" => {
            expect_file!["__snapshot__/github_bin_no_binding.yml"].assert_eq(&conf)
        }
        "github_pyproject_simple_targets.yml" => {
            expect_file!["__snapshot__/github_pyproject_simple_targets.yml"].assert_eq(&conf)
        }
        "github_pyproject_detailed_targets.yml" => {
            expect_file!["__snapshot__/github_pyproject_detailed_targets.yml"].assert_eq(&conf)
        }
        _ => panic!("unknown snapshot: {snapshot}"),
    }
}

fn pyo3_bridge(abi3: Option<Abi3Version>) -> BridgeModel {
    BridgeModel::PyO3(PyO3 {
        crate_name: PyO3Crate::PyO3,
        version: Version::new(0, 23, 0),
        abi3,
        metadata: None,
    })
}

fn target(arch: &str) -> TargetCIConfig {
    TargetCIConfig {
        arch: arch.to_string(),
        overrides: CIConfigOverrides::default(),
    }
}

fn render_with_config(
    cli: &GenerateCI,
    github_config: Option<&GitHubCIConfig>,
    bridge: &BridgeModel,
    sdist: bool,
) -> String {
    let resolved = resolve_config(cli, github_config, bridge).unwrap();
    generate_github(cli, &resolved, PROJECT_NAME, bridge, sdist).unwrap()
}

#[test]
fn test_generate_github() {
    let conf = generate_github_from_cli(
        &GenerateCI::default(),
        PROJECT_NAME,
        &pyo3_bridge(None),
        true,
    )
    .unwrap();
    assert_snapshot(&conf, "github_default.yml");
}

#[test]
fn test_generate_github_abi3() {
    let conf = generate_github_from_cli(
        &GenerateCI::default(),
        PROJECT_NAME,
        &pyo3_bridge(Some(Abi3Version::Version(3, 7))),
        false,
    )
    .unwrap();
    assert_snapshot(&conf, "github_abi3.yml");
}

#[test]
fn test_generate_github_no_attestations() {
    let cli = GenerateCI {
        skip_attestation: true,
        ..Default::default()
    };
    let conf = generate_github_from_cli(
        &cli,
        PROJECT_NAME,
        &pyo3_bridge(Some(Abi3Version::Version(3, 7))),
        false,
    )
    .unwrap();
    assert_snapshot(&conf, "github_no_attestations.yml");
}

#[test]
fn test_generate_github_zig_pytest() {
    let r#gen = GenerateCI {
        zig: true,
        pytest: true,
        ..Default::default()
    };
    let conf = generate_github_from_cli(&r#gen, PROJECT_NAME, &pyo3_bridge(None), true).unwrap();
    assert_snapshot(&conf, "github_zig_pytest.yml");
}

#[test]
fn test_generate_github_bin_no_binding() {
    let conf = generate_github_from_cli(
        &GenerateCI::default(),
        PROJECT_NAME,
        &BridgeModel::Bin(None),
        true,
    )
    .unwrap();
    assert_snapshot(&conf, "github_bin_no_binding.yml");
}

#[test]
fn test_generate_github_bin_skips_cli_emscripten() {
    let cli = GenerateCI {
        platforms: vec![Platform::Emscripten],
        ..Default::default()
    };
    let resolved = resolve_config(&cli, None, &BridgeModel::Bin(None)).unwrap();

    assert!(
        !resolved
            .platform_targets
            .contains_key(&Platform::Emscripten)
    );
}

#[test]
fn test_generate_github_pyproject_simple_targets() {
    let github_config = GitHubCIConfig {
        pytest: Some(false),
        zig: Some(false),
        skip_attestation: None,
        linux: Some(PlatformCIConfig {
            targets: Some(vec!["x86_64".to_string(), "aarch64".to_string()]),
            ..Default::default()
        }),
        macos: Some(PlatformCIConfig {
            targets: Some(vec!["aarch64".to_string()]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let conf = render_with_config(
        &GenerateCI::default(),
        Some(&github_config),
        &pyo3_bridge(None),
        true,
    );
    assert_snapshot(&conf, "github_pyproject_simple_targets.yml");
}

#[test]
fn test_generate_github_pyproject_detailed_targets() {
    let mut aarch64 = target("aarch64");
    aarch64.overrides.runner = Some("self-hosted-arm64".to_string());
    aarch64.overrides.manylinux = Some("2_17".to_string());
    aarch64.overrides.before_script_linux = Some("yum install -y openssl-devel".to_string());

    let github_config = GitHubCIConfig {
        pytest: None,
        zig: None,
        skip_attestation: None,
        linux: Some(PlatformCIConfig {
            overrides: CIConfigOverrides {
                runner: Some("ubuntu-22.04".to_string()),
                manylinux: Some("2_28".to_string()),
                ..Default::default()
            },
            target: Some(vec![target("x86_64"), aarch64]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let conf = render_with_config(
        &GenerateCI::default(),
        Some(&github_config),
        &pyo3_bridge(None),
        false,
    );
    assert_snapshot(&conf, "github_pyproject_detailed_targets.yml");
}

#[test]
fn test_generate_github_pyproject_cli_override() {
    let github_config = GitHubCIConfig {
        pytest: Some(true),
        zig: Some(true),
        skip_attestation: Some(true),
        linux: Some(PlatformCIConfig {
            targets: Some(vec!["x86_64".to_string()]),
            ..Default::default()
        }),
        macos: Some(PlatformCIConfig {
            targets: Some(vec!["aarch64".to_string()]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let cli = GenerateCI {
        platforms: vec![Platform::Windows],
        ..Default::default()
    };
    let bridge = BridgeModel::Bin(None);
    let resolved = resolve_config(&cli, Some(&github_config), &bridge).unwrap();

    assert!(resolved.platform_targets.contains_key(&Platform::Windows));
    assert!(!resolved.platform_targets.contains_key(&Platform::ManyLinux));
    assert!(!resolved.platform_targets.contains_key(&Platform::Macos));
    assert!(resolved.pytest);
    assert!(resolved.zig);
    assert!(resolved.skip_attestation);
}

#[test]
fn test_generate_github_pyproject_booleans_from_config() {
    let github_config = GitHubCIConfig {
        pytest: Some(true),
        zig: Some(true),
        skip_attestation: Some(true),
        ..Default::default()
    };

    let cli = GenerateCI::default();
    let bridge = BridgeModel::Bin(None);
    let resolved = resolve_config(&cli, Some(&github_config), &bridge).unwrap();

    assert!(resolved.pytest);
    assert!(resolved.zig);
    assert!(resolved.skip_attestation);
}

#[test]
fn test_generate_github_pyproject_cli_bool_override() {
    let github_config = GitHubCIConfig {
        pytest: Some(false),
        zig: Some(false),
        skip_attestation: Some(false),
        ..Default::default()
    };

    let cli = GenerateCI {
        pytest: true,
        zig: true,
        skip_attestation: true,
        ..Default::default()
    };
    let bridge = BridgeModel::Bin(None);
    let resolved = resolve_config(&cli, Some(&github_config), &bridge).unwrap();

    assert!(resolved.pytest);
    assert!(resolved.zig);
    assert!(resolved.skip_attestation);
}

#[test]
fn test_generate_github_pyproject_mutual_exclusion_error() {
    let github_config = GitHubCIConfig {
        linux: Some(PlatformCIConfig {
            targets: Some(vec!["x86_64".to_string()]),
            target: Some(vec![target("x86_64")]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let cli = GenerateCI::default();
    let bridge = BridgeModel::Bin(None);
    let result = resolve_config(&cli, Some(&github_config), &bridge);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("mutually exclusive"),
        "Expected mutual exclusion error, got: {err_msg}"
    );
}

#[test]
fn test_generate_github_pyproject_platform_level_runner() {
    let github_config = GitHubCIConfig {
        linux: Some(PlatformCIConfig {
            overrides: CIConfigOverrides {
                runner: Some("self-hosted-linux".to_string()),
                ..Default::default()
            },
            targets: Some(vec!["x86_64".to_string(), "aarch64".to_string()]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let cli = GenerateCI::default();
    let bridge = BridgeModel::Bin(None);
    let resolved = resolve_config(&cli, Some(&github_config), &bridge).unwrap();
    let targets = &resolved.platform_targets[&Platform::ManyLinux];

    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].runner, "self-hosted-linux");
    assert_eq!(targets[1].runner, "self-hosted-linux");
}

#[test]
fn test_generate_github_pyproject_uniform_manylinux() {
    let github_config = GitHubCIConfig {
        linux: Some(PlatformCIConfig {
            overrides: CIConfigOverrides {
                manylinux: Some("2_28".to_string()),
                ..Default::default()
            },
            targets: Some(vec!["x86_64".to_string(), "aarch64".to_string()]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let cli = GenerateCI::default();
    let bridge = pyo3_bridge(None);
    let resolved = resolve_config(&cli, Some(&github_config), &bridge).unwrap();
    let conf = generate_github(&cli, &resolved, PROJECT_NAME, &bridge, false).unwrap();

    assert!(conf.contains("          manylinux: 2_28\n"));
    assert!(!conf.contains("matrix.platform.manylinux"));
}

#[test]
fn test_generate_github_android() {
    let github_config = GitHubCIConfig {
        android: Some(PlatformCIConfig::default()),
        ..Default::default()
    };

    let conf = render_with_config(
        &GenerateCI::default(),
        Some(&github_config),
        &pyo3_bridge(None),
        false,
    );

    assert!(conf.contains("  android:\n"));
    assert!(conf.contains("runner: ubuntu-latest"));
    assert!(conf.contains("target: aarch64-linux-android"));
    assert!(conf.contains("target: x86_64-linux-android"));
    assert!(!conf.contains("actions/setup-python"));
    assert!(!conf.contains("--find-interpreter"));
    assert!(!conf.contains("manylinux:"));
}

#[test]
fn test_generate_github_android_bin() {
    let github_config = GitHubCIConfig {
        android: Some(PlatformCIConfig::default()),
        linux: Some(PlatformCIConfig::default()),
        ..Default::default()
    };

    let conf = render_with_config(
        &GenerateCI::default(),
        Some(&github_config),
        &BridgeModel::Bin(None),
        false,
    );

    assert!(conf.contains("  android:\n"));
    assert!(conf.contains("  linux:\n"));
}
