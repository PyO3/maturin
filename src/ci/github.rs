use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Result, bail};

use super::{GenerateCI, Platform, ResolvedCIConfig, ResolvedTarget};
use crate::BridgeModel;
use crate::pyproject_toml::{GitHubCIConfig, PlatformCIConfig};

/// Check if a given maturin-action field is uniform across all targets in the resolved list.
/// Returns Some(value) if all targets have the same value (or all None), None if they vary.
fn uniform_field<F: Fn(&ResolvedTarget) -> Option<&str>>(
    targets: &[ResolvedTarget],
    accessor: F,
) -> Option<Option<String>> {
    if targets.is_empty() {
        return Some(None);
    }
    let first = accessor(&targets[0]);
    for t in &targets[1..] {
        let val = accessor(t);
        if val != first {
            return None; // Varying
        }
    }
    Some(first.map(|s| s.to_string()))
}

type FieldAccessor = fn(&ResolvedTarget) -> Option<&str>;

/// The list of maturin-action fields that can be configured per-target.
/// Each entry is (action_field_name with hyphens, matrix_key with underscores, accessor).
const MATURIN_ACTION_FIELDS: &[(&str, &str, FieldAccessor)] = &[
    ("manylinux", "manylinux", |t| t.manylinux.as_deref()),
    ("container", "container", |t| t.container.as_deref()),
    ("docker-options", "docker_options", |t| {
        t.docker_options.as_deref()
    }),
    ("rust-toolchain", "rust_toolchain", |t| {
        t.rust_toolchain.as_deref()
    }),
    ("rustup-components", "rustup_components", |t| {
        t.rustup_components.as_deref()
    }),
    ("before-script-linux", "before_script_linux", |t| {
        t.before_script_linux.as_deref()
    }),
];

/// Emit a maturin-action field either at step level (uniform) or as matrix reference.
fn emit_maturin_action_field(
    conf: &mut String,
    field_name: &str,
    matrix_key: &str,
    targets: &[ResolvedTarget],
    accessor: FieldAccessor,
) {
    if let Some(uniform_val) = uniform_field(targets, accessor) {
        // Uniform: emit at step level if value exists
        if let Some(val) = uniform_val {
            conf.push_str(&format!("          {field_name}: {val}\n"));
        }
    } else {
        // Varying: will be emitted via matrix reference
        conf.push_str(&format!(
            "          {field_name}: ${{{{ matrix.platform.{matrix_key} }}}}\n",
        ));
    }
}

/// Returns the hardcoded default runner for a given platform and architecture.
fn default_runner(platform: Platform, arch: &str) -> &'static str {
    match platform {
        Platform::ManyLinux | Platform::Musllinux | Platform::Emscripten => "ubuntu-22.04",
        Platform::Android => "ubuntu-latest",
        Platform::Windows => {
            if arch == "aarch64" {
                "windows-11-arm"
            } else {
                "windows-latest"
            }
        }
        Platform::Macos => {
            if arch == "x86_64" {
                "macos-15-intel"
            } else {
                "macos-latest"
            }
        }
        Platform::All => "ubuntu-22.04",
    }
}

/// Returns the hardcoded default python_arch for a given platform and architecture.
fn default_python_arch(platform: Platform, arch: &str) -> Option<String> {
    match platform {
        Platform::Windows => {
            if arch == "aarch64" {
                Some("arm64".to_string())
            } else {
                Some(arch.to_string())
            }
        }
        _ => None,
    }
}

/// Returns the hardcoded default target list for a given platform.
fn default_targets(platform: Platform) -> Vec<&'static str> {
    match platform {
        Platform::ManyLinux => vec!["x86_64", "x86", "aarch64", "armv7", "s390x", "ppc64le"],
        Platform::Musllinux => vec!["x86_64", "x86", "aarch64", "armv7"],
        Platform::Windows => vec!["x64", "x86", "aarch64"],
        Platform::Macos => vec!["x86_64", "aarch64"],
        Platform::Emscripten => vec!["wasm32-unknown-emscripten"],
        Platform::Android => vec!["aarch64-linux-android", "x86_64-linux-android"],
        Platform::All => vec![],
    }
}

/// Returns the hardcoded default manylinux value for a given platform.
fn default_manylinux(platform: Platform) -> Option<&'static str> {
    match platform {
        Platform::ManyLinux => Some("auto"),
        Platform::Musllinux => Some("musllinux_1_2"),
        _ => None,
    }
}

/// Returns the hardcoded default rust-toolchain for a given platform.
fn default_rust_toolchain(platform: Platform) -> Option<&'static str> {
    match platform {
        Platform::Emscripten => Some("nightly"),
        _ => None,
    }
}

/// Resolve a field using the chain: per-target → platform-level → default.
fn resolve_optional(
    per_target: Option<&str>,
    platform_level: Option<&str>,
    default: Option<&str>,
) -> Option<String> {
    per_target
        .or(platform_level)
        .or(default)
        .map(|s| s.to_string())
}

/// Validate a PlatformCIConfig: `targets` and `target` are mutually exclusive.
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

/// Resolve targets for a given platform from pyproject config + hardcoded defaults.
pub(crate) fn resolve_platform_targets(
    platform: Platform,
    platform_config: Option<&PlatformCIConfig>,
    github_config: Option<&GitHubCIConfig>,
) -> Result<Vec<ResolvedTarget>> {
    let plat_name = platform.to_string();
    if let Some(config) = platform_config {
        validate_platform_config(&plat_name, config)?;
    }

    // Determine arch list
    let arch_list: Vec<String> = match platform_config {
        Some(config) if config.target.is_some() => config
            .target
            .as_ref()
            .unwrap()
            .iter()
            .map(|t| t.arch.clone())
            .collect(),
        Some(config) if config.targets.is_some() => config.targets.clone().unwrap(),
        _ => default_targets(platform)
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };

    let mut resolved = Vec::new();
    for arch in &arch_list {
        let python_arch = default_python_arch(platform, arch);

        // Find per-target config if using detailed form
        let per_target = platform_config.and_then(|c| {
            c.target
                .as_ref()
                .and_then(|targets| targets.iter().find(|t| &t.arch == arch))
        });

        // Shorthand accessors for the resolution chain
        let pt = |f: fn(&crate::pyproject_toml::TargetCIConfig) -> &Option<String>| {
            per_target.and_then(|t| f(t).as_deref())
        };
        let pl = |f: fn(&PlatformCIConfig) -> &Option<String>| {
            platform_config.and_then(|c| f(c).as_deref())
        };

        let runner = resolve_optional(
            pt(|t| &t.runner),
            pl(|c| &c.runner),
            Some(default_runner(platform, arch)),
        )
        .unwrap();

        resolved.push(ResolvedTarget {
            runner,
            target: arch.clone(),
            python_arch,
            manylinux: resolve_optional(
                pt(|t| &t.manylinux),
                pl(|c| &c.manylinux),
                default_manylinux(platform),
            ),
            container: resolve_optional(pt(|t| &t.container), pl(|c| &c.container), None),
            docker_options: resolve_optional(
                pt(|t| &t.docker_options),
                pl(|c| &c.docker_options),
                None,
            ),
            rust_toolchain: resolve_optional(
                pt(|t| &t.rust_toolchain),
                pl(|c| &c.rust_toolchain),
                default_rust_toolchain(platform),
            ),
            rustup_components: resolve_optional(
                pt(|t| &t.rustup_components),
                pl(|c| &c.rustup_components),
                None,
            ),
            before_script_linux: resolve_optional(
                pt(|t| &t.before_script_linux),
                pl(|c| &c.before_script_linux),
                None,
            ),
            extra_args: resolve_optional(
                pt(|t| &t.args),
                pl(|c| &c.args),
                github_config.and_then(|c| c.args.as_deref()),
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
    // Booleans: CLI flags override pyproject (CLI flag = true wins, else pyproject, else false)
    let pytest = cli.pytest || github_config.and_then(|c| c.pytest).unwrap_or(false);
    let zig = cli.zig || github_config.and_then(|c| c.zig).unwrap_or(false);
    let skip_attestation = cli.skip_attestation
        || github_config
            .and_then(|c| c.skip_attestation)
            .unwrap_or(false);

    // Platform selection: CLI --platform wins, else pyproject platform presence, else defaults
    let cli_has_platforms = !cli.platforms.is_empty();

    let platforms: BTreeSet<Platform> = if cli_has_platforms {
        // CLI specified platforms
        cli.platforms
            .iter()
            .flat_map(|p| {
                if matches!(p, Platform::All) {
                    if !bridge_model.is_bin() {
                        Platform::all()
                    } else {
                        Platform::defaults()
                    }
                } else {
                    std::slice::from_ref(p)
                }
            })
            .filter(|p| !bridge_model.is_bin() || !matches!(p, Platform::Emscripten))
            .copied()
            .collect()
    } else if let Some(gh) = github_config {
        // Check if any platform sub-tables exist
        let has_platform_config = gh.linux.is_some()
            || gh.musllinux.is_some()
            || gh.windows.is_some()
            || gh.macos.is_some()
            || gh.emscripten.is_some()
            || gh.android.is_some();
        if has_platform_config {
            let mut plats = BTreeSet::new();
            if gh.linux.is_some() {
                plats.insert(Platform::ManyLinux);
            }
            if gh.musllinux.is_some() {
                plats.insert(Platform::Musllinux);
            }
            if gh.windows.is_some() {
                plats.insert(Platform::Windows);
            }
            if gh.macos.is_some() {
                plats.insert(Platform::Macos);
            }
            if gh.emscripten.is_some() && !bridge_model.is_bin() {
                plats.insert(Platform::Emscripten);
            }
            if gh.android.is_some() {
                plats.insert(Platform::Android);
            }
            plats
        } else {
            // No platform sub-tables: use defaults
            Platform::defaults().iter().copied().collect()
        }
    } else {
        // No pyproject config at all: use defaults
        Platform::defaults().iter().copied().collect()
    };

    // Resolve targets for each platform
    let mut platform_targets = BTreeMap::new();
    for &platform in &platforms {
        let platform_config = github_config.and_then(|gh| match platform {
            Platform::ManyLinux => gh.linux.as_ref(),
            Platform::Musllinux => gh.musllinux.as_ref(),
            Platform::Windows => gh.windows.as_ref(),
            Platform::Macos => gh.macos.as_ref(),
            Platform::Emscripten => gh.emscripten.as_ref(),
            Platform::Android => gh.android.as_ref(),
            Platform::All => None,
        });
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

pub(crate) fn generate_github_from_cli(
    cli: &GenerateCI,
    project_name: &str,
    bridge_model: &BridgeModel,
    sdist: bool,
) -> Result<String> {
    let resolved = resolve_config(cli, None, bridge_model)?;
    generate_github(cli, &resolved, project_name, bridge_model, sdist)
}

/// Generate GitHub Actions CI configuration.
pub(crate) fn generate_github(
    cli: &GenerateCI,
    resolved: &ResolvedCIConfig,
    project_name: &str,
    bridge_model: &BridgeModel,
    sdist: bool,
) -> Result<String> {
    let is_abi3 = bridge_model.is_abi3();
    let is_bin = bridge_model.is_bin();
    let setup_python = resolved.pytest
        || matches!(
            bridge_model,
            BridgeModel::Bin(Some(_))
                | BridgeModel::PyO3 { .. }
                | BridgeModel::Cffi
                | BridgeModel::UniFfi
        );
    let mut gen_cmd = std::env::args()
        .enumerate()
        .map(|(i, arg)| {
            if i == 0 {
                env!("CARGO_PKG_NAME").to_string()
            } else {
                arg
            }
        })
        .collect::<Vec<String>>()
        .join(" ");
    if gen_cmd.starts_with("maturin new") || gen_cmd.starts_with("maturin init") {
        gen_cmd = format!("{} generate-ci github", env!("CARGO_PKG_NAME"));
    }
    let mut conf = format!(
        "# This file is autogenerated by maturin v{version}
# To update, run
#
#    {gen_cmd}
#
name: CI

on:
  push:
    branches:
      - main
      - master
    tags:
      - '*'
  pull_request:
  workflow_dispatch:

permissions:
  contents: read

jobs:\n",
        version = env!("CARGO_PKG_VERSION"),
    );

    let mut needs = Vec::new();

    for (&platform, targets) in &resolved.platform_targets {
        let plat_name = platform.to_string();
        needs.push(plat_name.clone());
        conf.push_str(&format!(
            "  {plat_name}:
    runs-on: ${{{{ matrix.platform.runner }}}}\n"
        ));

        // target matrix
        if !targets.is_empty() {
            conf.push_str(
                "    strategy:
      matrix:
        platform:\n",
            );
        }
        for target in targets {
            conf.push_str(&format!(
                "          - runner: {}\n            target: {}\n",
                target.runner, target.target,
            ));
            if let Some(ref python_arch) = target.python_arch {
                conf.push_str(&format!("            python_arch: {}\n", python_arch));
            }
            // Emit varying maturin-action fields into the matrix
            emit_varying_matrix_fields(&mut conf, targets, target);
        }

        // job steps
        conf.push_str(
            "    steps:
      - uses: actions/checkout@v6\n",
        );

        // install pyodide-build for emscripten
        if matches!(platform, Platform::Emscripten) {
            conf.push_str("      - run: pip install pyodide-build\n");
            conf.push_str(
                "      - name: Get Emscripten and Python version info
        shell: bash
        run: |
          echo EMSCRIPTEN_VERSION=$(pyodide config get emscripten_version) >> $GITHUB_ENV
          echo PYTHON_VERSION=$(pyodide config get python_version | cut -d '.' -f 1-2) >> $GITHUB_ENV
          pip uninstall -y pyodide-build\n",
            );
            conf.push_str(
                "      - uses: mymindstorm/setup-emsdk@v14
        with:
          version: ${{ env.EMSCRIPTEN_VERSION }}
          actions-cache-folder: emsdk-cache\n",
            );
            conf.push_str(
                "      - uses: actions/setup-python@v6
        with:
          python-version: ${{ env.PYTHON_VERSION }}\n",
            );
            conf.push_str("      - run: pip install pyodide-build\n");
        } else if matches!(platform, Platform::Android) {
            // Android cross-builds don't need setup-python on the host
        } else {
            // setup python on demand
            if setup_python {
                let python_ver = if matches!(platform, Platform::Windows) {
                    "3.13"
                } else {
                    "3.x"
                };
                conf.push_str(&format!(
                    "      - uses: actions/setup-python@v6
        with:
          python-version: {python_ver}\n",
                ));
                if matches!(platform, Platform::Windows) {
                    conf.push_str("          architecture: ${{ matrix.platform.python_arch }}\n");
                }
            }
        }

        // build wheels
        let mut maturin_args = if is_abi3 || (is_bin && !setup_python) {
            Vec::new()
        } else if matches!(platform, Platform::Emscripten) {
            vec!["-i".to_string(), "${{ env.PYTHON_VERSION }}".to_string()]
        } else if matches!(platform, Platform::Android) {
            // Android cross-builds: no --find-interpreter
            Vec::new()
        } else {
            vec!["--find-interpreter".to_string()]
        };
        if let Some(manifest_path) = cli.manifest_path.as_ref()
            && manifest_path != Path::new("Cargo.toml")
        {
            maturin_args.push("--manifest-path".to_string());
            maturin_args.push(manifest_path.display().to_string())
        }
        if resolved.zig && matches!(platform, Platform::ManyLinux) {
            maturin_args.push("--zig".to_string());
        }
        // Resolve extra args: uniform = inline, varying = matrix reference
        let extra_args_suffix =
            if let Some(uniform_val) = uniform_field(targets, |t| t.extra_args.as_deref()) {
                uniform_val.map(|v| format!(" {v}")).unwrap_or_default()
            } else {
                " ${{ matrix.platform.extra_args }}".to_string()
            };

        let maturin_args = if maturin_args.is_empty() {
            String::new()
        } else {
            format!(" {}", maturin_args.join(" "))
        };
        conf.push_str(&format!(
            "      - name: Build wheels
        uses: PyO3/maturin-action@v1
        with:
          target: ${{{{ matrix.platform.target }}}}
          args: --release --out dist{maturin_args}{extra_args_suffix}
          sccache: ${{{{ !startsWith(github.ref, 'refs/tags/') }}}}
"
        ));

        // Emit maturin-action options (smart: step-level if uniform, matrix ref if varying)
        for &(field_name, matrix_key, accessor) in MATURIN_ACTION_FIELDS {
            emit_maturin_action_field(&mut conf, field_name, matrix_key, targets, accessor);
        }

        if is_abi3 {
            // build free-threaded wheel for python3.14t
            if matches!(platform, Platform::Windows) {
                conf.push_str(
                    "      - uses: actions/setup-python@v6
        with:
          python-version: 3.14t\n",
                );
                conf.push_str("          architecture: ${{ matrix.platform.python_arch }}\n");
            }
            conf.push_str(&format!(
                "      - name: Build free-threaded wheels
        uses: PyO3/maturin-action@v1
        with:
          target: ${{{{ matrix.platform.target }}}}
          args: --release --out dist{maturin_args}{extra_args_suffix} -i python3.14t
          sccache: ${{{{ !startsWith(github.ref, 'refs/tags/') }}}}
"
            ));
            // Emit same maturin-action options for free-threaded build
            for &(field_name, matrix_key, accessor) in MATURIN_ACTION_FIELDS {
                emit_maturin_action_field(&mut conf, field_name, matrix_key, targets, accessor);
            }
        }

        // upload wheels
        let artifact_name = match platform {
            Platform::Emscripten => "wasm-wheels".to_string(),
            _ => format!("wheels-{platform}-${{{{ matrix.platform.target }}}}"),
        };
        conf.push_str(&format!(
            "      - name: Upload wheels
        uses: actions/upload-artifact@v6
        with:
          name: {artifact_name}
          path: dist
"
        ));

        // pytest
        let mut chdir = String::new();
        if let Some(manifest_path) = cli.manifest_path.as_ref()
            && manifest_path != Path::new("Cargo.toml")
        {
            let parent = manifest_path.parent().unwrap();
            chdir = format!("cd {} && ", parent.display());
        }
        if resolved.pytest {
            match platform {
                Platform::All | Platform::Android => {}
                Platform::ManyLinux => {
                    // Test on host for x86_64 GNU targets
                    conf.push_str(
                        "      - uses: astral-sh/setup-uv@v7
        if: ${{ startsWith(matrix.platform.target, 'x86_64') }}
",
                    );
                    conf.push_str(&format!(
                        "      - name: pytest
        if: ${{{{ startsWith(matrix.platform.target, 'x86_64') }}}}
        shell: bash
        run: |
          set -e
          uv venv .venv
          source .venv/bin/activate
          uv pip install {project_name} --no-index --no-deps --find-links dist --reinstall
          uv pip install {project_name} pytest
          {chdir}pytest
"
                    ));
                    // Test on QEMU for other GNU architectures
                    conf.push_str(&format!(
                        "      - name: pytest
        if: ${{{{ !startsWith(matrix.platform.target, 'x86') && matrix.platform.target != 'ppc64' }}}}
        uses: uraimo/run-on-arch-action@v2
        with:
          arch: ${{{{ matrix.platform.target }}}}
          distro: ubuntu22.04
          githubToken: ${{{{ github.token }}}}
          install: |
            apt-get update
            apt-get install -y --no-install-recommends python3 python3-pip
            pip3 install -U pip pytest
          run: |
            set -e
            pip3 install {project_name} --no-index --no-deps --find-links dist --force-reinstall
            pip3 install {project_name}
            {chdir}pytest
"
                    ));
                }
                Platform::Musllinux => {
                    conf.push_str(&format!(
                        "      - name: pytest
        if: ${{{{ startsWith(matrix.platform.target, 'x86_64') }}}}
        run: |
          set -e
          docker run --rm -v ${{{{ github.workspace }}}}:/io -w /io alpine:latest sh -c '
            apk add py3-pip py3-virtualenv
            python3 -m virtualenv .venv
            source .venv/bin/activate
            pip install {project_name} --no-index --no-deps --find-links dist --force-reinstall
            pip install {project_name} pytest
            {chdir}pytest
          '
"
                    ));
                    conf.push_str(&format!(
                        "      - name: pytest
        if: ${{{{ !startsWith(matrix.platform.target, 'x86') }}}}
        uses: uraimo/run-on-arch-action@v2
        with:
          arch: ${{{{ matrix.platform.target }}}}
          distro: alpine_latest
          githubToken: ${{{{ github.token }}}}
          install: |
            apk add py3-virtualenv
          run: |
            set -e
            python3 -m virtualenv .venv
            source .venv/bin/activate
            pip install {project_name} --no-index --no-deps --find-links dist --force-reinstall
            pip install {project_name} pytest
            {chdir}pytest
"
                    ));
                }
                Platform::Windows => {
                    conf.push_str(
                        "      - uses: astral-sh/setup-uv@v7
",
                    );
                    conf.push_str(&format!(
                        "      - name: pytest
        shell: bash
        run: |
          set -e
          uv venv .venv
          source .venv/Scripts/activate
          uv pip install {project_name} --no-index --no-deps --find-links dist --reinstall
          uv pip install {project_name} pytest
          {chdir}pytest
"
                    ));
                }
                Platform::Macos => {
                    conf.push_str(
                        "      - uses: astral-sh/setup-uv@v7
",
                    );
                    conf.push_str(&format!(
                        "      - name: pytest
        run: |
          set -e
          uv venv .venv
          source .venv/bin/activate
          uv pip install {project_name} --no-index --no-deps --find-links dist --reinstall
          uv pip install {project_name} pytest
          {chdir}pytest
"
                    ));
                }
                Platform::Emscripten => {
                    conf.push_str(
                        "      - uses: actions/setup-node@v3
        with:
          node-version: '22'
",
                    );
                    conf.push_str(&format!(
                        "      - name: pytest
        run: |
          set -e
          pyodide venv .venv
          source .venv/bin/activate
          pip install {project_name} --no-index --no-deps --find-links dist --force-reinstall
          pip install {project_name} pytest
          {chdir}python -m pytest
"
                    ));
                }
            }
        }

        conf.push('\n');
    }

    // build sdist
    if sdist {
        needs.push("sdist".to_string());

        let maturin_args = cli
            .manifest_path
            .as_ref()
            .map(|manifest_path| {
                if manifest_path != Path::new("Cargo.toml") {
                    format!(" --manifest-path {}", manifest_path.display())
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();
        conf.push_str(&format!(
            r#"  sdist:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - name: Build sdist
        uses: PyO3/maturin-action@v1
        with:
          command: sdist
          args: --out dist{maturin_args}
"#
        ));
        conf.push_str(
            "      - name: Upload sdist
        uses: actions/upload-artifact@v6
        with:
          name: wheels-sdist
          path: dist
",
        );
        conf.push('\n');
    }

    conf.push_str(&format!(
        r#"  release:
    name: Release
    runs-on: ubuntu-latest
    if: ${{{{ startsWith(github.ref, 'refs/tags/') || github.event_name == 'workflow_dispatch' }}}}
    needs: [{needs}]
"#,
        needs = needs.join(", ")
    ));

    conf.push_str(
        r#"    permissions:
      # Use to sign the release artifacts
      id-token: write
      # Used to upload release artifacts
      contents: write
"#,
    );
    if !resolved.skip_attestation {
        conf.push_str(
            r#"      # Used to generate artifact attestation
      attestations: write
"#,
        );
    }
    conf.push_str(
        r#"    steps:
      - uses: actions/download-artifact@v7
"#,
    );
    if !resolved.skip_attestation {
        conf.push_str(
            r#"      - name: Generate artifact attestation
        uses: actions/attest-build-provenance@v3
        with:
          subject-path: 'wheels-*/*'
"#,
        );
    }
    conf.push_str(
        r#"      - name: Install uv
        if: ${{ startsWith(github.ref, 'refs/tags/') }}
        uses: astral-sh/setup-uv@v7
"#,
    );
    conf.push_str(
        r#"      - name: Publish to PyPI
        if: ${{ startsWith(github.ref, 'refs/tags/') }}
        run: uv publish 'wheels-*/*'
        env:
          UV_PUBLISH_TOKEN: ${{ secrets.PYPI_API_TOKEN }}
"#,
    );
    if resolved
        .platform_targets
        .contains_key(&Platform::Emscripten)
    {
        conf.push_str(
            "      - name: Upload to GitHub Release
        uses: softprops/action-gh-release@v1
        with:
          files: |
            wasm-wheels/*.whl
          prerelease: ${{ contains(github.ref, 'alpha') || contains(github.ref, 'beta') }}
",
        );
    }
    Ok(conf)
}

/// Emit varying maturin-action fields into matrix entries for a specific target.
fn emit_varying_matrix_fields(
    conf: &mut String,
    all_targets: &[ResolvedTarget],
    target: &ResolvedTarget,
) {
    for &(_field_name, matrix_key, accessor) in MATURIN_ACTION_FIELDS {
        if uniform_field(all_targets, accessor).is_none() {
            // Field varies — emit it for this target
            if let Some(val) = accessor(target) {
                conf.push_str(&format!("            {matrix_key}: {val}\n"));
            }
        }
    }
    // extra_args is handled separately (appended to the args: line, not a separate with: key)
    if uniform_field(all_targets, |t| t.extra_args.as_deref()).is_none()
        && let Some(ref val) = target.extra_args
    {
        conf.push_str(&format!("            extra_args: {val}\n"));
    }
}

#[cfg(test)]
mod tests {
    use crate::ci::{GenerateCI, Platform};
    use crate::{Abi3Version, BridgeModel, PyO3, bridge::PyO3Crate};
    use expect_test::expect;
    use semver::Version;

    #[test]
    fn test_generate_github() {
        let conf = super::generate_github_from_cli(
            &GenerateCI::default(),
            "example",
            &BridgeModel::PyO3(PyO3 {
                crate_name: PyO3Crate::PyO3,
                version: Version::new(0, 23, 0),
                abi3: None,
                metadata: None,
            }),
            true,
        )
        .unwrap()
        .lines()
        .skip(5)
        .collect::<Vec<_>>()
        .join("\n");
        let expected = expect![[r#"
            name: CI

            on:
              push:
                branches:
                  - main
                  - master
                tags:
                  - '*'
              pull_request:
              workflow_dispatch:

            permissions:
              contents: read

            jobs:
              linux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: x86
                      - runner: ubuntu-22.04
                        target: aarch64
                      - runner: ubuntu-22.04
                        target: armv7
                      - runner: ubuntu-22.04
                        target: s390x
                      - runner: ubuntu-22.04
                        target: ppc64le
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: auto
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-linux-${{ matrix.platform.target }}
                      path: dist

              musllinux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: x86
                      - runner: ubuntu-22.04
                        target: aarch64
                      - runner: ubuntu-22.04
                        target: armv7
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: musllinux_1_2
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-musllinux-${{ matrix.platform.target }}
                      path: dist

              windows:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: windows-latest
                        target: x64
                        python_arch: x64
                      - runner: windows-latest
                        target: x86
                        python_arch: x86
                      - runner: windows-11-arm
                        target: aarch64
                        python_arch: arm64
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.13
                      architecture: ${{ matrix.platform.python_arch }}
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-windows-${{ matrix.platform.target }}
                      path: dist

              macos:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: macos-15-intel
                        target: x86_64
                      - runner: macos-latest
                        target: aarch64
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-macos-${{ matrix.platform.target }}
                      path: dist

              sdist:
                runs-on: ubuntu-latest
                steps:
                  - uses: actions/checkout@v6
                  - name: Build sdist
                    uses: PyO3/maturin-action@v1
                    with:
                      command: sdist
                      args: --out dist
                  - name: Upload sdist
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-sdist
                      path: dist

              release:
                name: Release
                runs-on: ubuntu-latest
                if: ${{ startsWith(github.ref, 'refs/tags/') || github.event_name == 'workflow_dispatch' }}
                needs: [linux, musllinux, windows, macos, sdist]
                permissions:
                  # Use to sign the release artifacts
                  id-token: write
                  # Used to upload release artifacts
                  contents: write
                  # Used to generate artifact attestation
                  attestations: write
                steps:
                  - uses: actions/download-artifact@v7
                  - name: Generate artifact attestation
                    uses: actions/attest-build-provenance@v3
                    with:
                      subject-path: 'wheels-*/*'
                  - name: Install uv
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    uses: astral-sh/setup-uv@v7
                  - name: Publish to PyPI
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    run: uv publish 'wheels-*/*'
                    env:
                      UV_PUBLISH_TOKEN: ${{ secrets.PYPI_API_TOKEN }}"#]];
        expected.assert_eq(&conf);
    }

    #[test]
    fn test_generate_github_abi3() {
        let conf = super::generate_github_from_cli(
            &GenerateCI::default(),
            "example",
            &BridgeModel::PyO3(PyO3 {
                crate_name: PyO3Crate::PyO3,
                version: Version::new(0, 23, 0),
                abi3: Some(Abi3Version::Version(3, 7)),
                metadata: None,
            }),
            false,
        )
        .unwrap()
        .lines()
        .skip(5)
        .collect::<Vec<_>>()
        .join("\n");
        let expected = expect![[r#"
            name: CI

            on:
              push:
                branches:
                  - main
                  - master
                tags:
                  - '*'
              pull_request:
              workflow_dispatch:

            permissions:
              contents: read

            jobs:
              linux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: x86
                      - runner: ubuntu-22.04
                        target: aarch64
                      - runner: ubuntu-22.04
                        target: armv7
                      - runner: ubuntu-22.04
                        target: s390x
                      - runner: ubuntu-22.04
                        target: ppc64le
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: auto
                  - name: Build free-threaded wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist -i python3.14t
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: auto
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-linux-${{ matrix.platform.target }}
                      path: dist

              musllinux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: x86
                      - runner: ubuntu-22.04
                        target: aarch64
                      - runner: ubuntu-22.04
                        target: armv7
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: musllinux_1_2
                  - name: Build free-threaded wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist -i python3.14t
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: musllinux_1_2
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-musllinux-${{ matrix.platform.target }}
                      path: dist

              windows:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: windows-latest
                        target: x64
                        python_arch: x64
                      - runner: windows-latest
                        target: x86
                        python_arch: x86
                      - runner: windows-11-arm
                        target: aarch64
                        python_arch: arm64
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.13
                      architecture: ${{ matrix.platform.python_arch }}
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.14t
                      architecture: ${{ matrix.platform.python_arch }}
                  - name: Build free-threaded wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist -i python3.14t
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-windows-${{ matrix.platform.target }}
                      path: dist

              macos:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: macos-15-intel
                        target: x86_64
                      - runner: macos-latest
                        target: aarch64
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Build free-threaded wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist -i python3.14t
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-macos-${{ matrix.platform.target }}
                      path: dist

              release:
                name: Release
                runs-on: ubuntu-latest
                if: ${{ startsWith(github.ref, 'refs/tags/') || github.event_name == 'workflow_dispatch' }}
                needs: [linux, musllinux, windows, macos]
                permissions:
                  # Use to sign the release artifacts
                  id-token: write
                  # Used to upload release artifacts
                  contents: write
                  # Used to generate artifact attestation
                  attestations: write
                steps:
                  - uses: actions/download-artifact@v7
                  - name: Generate artifact attestation
                    uses: actions/attest-build-provenance@v3
                    with:
                      subject-path: 'wheels-*/*'
                  - name: Install uv
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    uses: astral-sh/setup-uv@v7
                  - name: Publish to PyPI
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    run: uv publish 'wheels-*/*'
                    env:
                      UV_PUBLISH_TOKEN: ${{ secrets.PYPI_API_TOKEN }}"#]];
        expected.assert_eq(&conf);
    }

    #[test]
    fn test_generate_github_no_attestations() {
        let cli = GenerateCI {
            skip_attestation: true,
            ..Default::default()
        };
        let conf = super::generate_github_from_cli(
            &cli,
            "example",
            &BridgeModel::PyO3(PyO3 {
                crate_name: PyO3Crate::PyO3,
                version: Version::new(0, 23, 0),
                abi3: Some(Abi3Version::Version(3, 7)),
                metadata: None,
            }),
            false,
        )
        .unwrap()
        .lines()
        .skip(5)
        .collect::<Vec<_>>()
        .join("\n");
        let expected = expect![[r#"
            name: CI

            on:
              push:
                branches:
                  - main
                  - master
                tags:
                  - '*'
              pull_request:
              workflow_dispatch:

            permissions:
              contents: read

            jobs:
              linux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: x86
                      - runner: ubuntu-22.04
                        target: aarch64
                      - runner: ubuntu-22.04
                        target: armv7
                      - runner: ubuntu-22.04
                        target: s390x
                      - runner: ubuntu-22.04
                        target: ppc64le
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: auto
                  - name: Build free-threaded wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist -i python3.14t
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: auto
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-linux-${{ matrix.platform.target }}
                      path: dist

              musllinux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: x86
                      - runner: ubuntu-22.04
                        target: aarch64
                      - runner: ubuntu-22.04
                        target: armv7
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: musllinux_1_2
                  - name: Build free-threaded wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist -i python3.14t
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: musllinux_1_2
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-musllinux-${{ matrix.platform.target }}
                      path: dist

              windows:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: windows-latest
                        target: x64
                        python_arch: x64
                      - runner: windows-latest
                        target: x86
                        python_arch: x86
                      - runner: windows-11-arm
                        target: aarch64
                        python_arch: arm64
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.13
                      architecture: ${{ matrix.platform.python_arch }}
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.14t
                      architecture: ${{ matrix.platform.python_arch }}
                  - name: Build free-threaded wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist -i python3.14t
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-windows-${{ matrix.platform.target }}
                      path: dist

              macos:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: macos-15-intel
                        target: x86_64
                      - runner: macos-latest
                        target: aarch64
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Build free-threaded wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist -i python3.14t
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-macos-${{ matrix.platform.target }}
                      path: dist

              release:
                name: Release
                runs-on: ubuntu-latest
                if: ${{ startsWith(github.ref, 'refs/tags/') || github.event_name == 'workflow_dispatch' }}
                needs: [linux, musllinux, windows, macos]
                permissions:
                  # Use to sign the release artifacts
                  id-token: write
                  # Used to upload release artifacts
                  contents: write
                steps:
                  - uses: actions/download-artifact@v7
                  - name: Install uv
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    uses: astral-sh/setup-uv@v7
                  - name: Publish to PyPI
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    run: uv publish 'wheels-*/*'
                    env:
                      UV_PUBLISH_TOKEN: ${{ secrets.PYPI_API_TOKEN }}"#]];
        expected.assert_eq(&conf);
    }

    #[test]
    fn test_generate_github_zig_pytest() {
        let r#gen = GenerateCI {
            zig: true,
            pytest: true,
            ..Default::default()
        };
        let conf = super::generate_github_from_cli(
            &r#gen,
            "example",
            &BridgeModel::PyO3(PyO3 {
                crate_name: PyO3Crate::PyO3,
                version: Version::new(0, 23, 0),
                abi3: None,
                metadata: None,
            }),
            true,
        )
        .unwrap()
        .lines()
        .skip(5)
        .collect::<Vec<_>>()
        .join("\n");
        let expected = expect![[r#"
            name: CI

            on:
              push:
                branches:
                  - main
                  - master
                tags:
                  - '*'
              pull_request:
              workflow_dispatch:

            permissions:
              contents: read

            jobs:
              linux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: x86
                      - runner: ubuntu-22.04
                        target: aarch64
                      - runner: ubuntu-22.04
                        target: armv7
                      - runner: ubuntu-22.04
                        target: s390x
                      - runner: ubuntu-22.04
                        target: ppc64le
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter --zig
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: auto
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-linux-${{ matrix.platform.target }}
                      path: dist
                  - uses: astral-sh/setup-uv@v7
                    if: ${{ startsWith(matrix.platform.target, 'x86_64') }}
                  - name: pytest
                    if: ${{ startsWith(matrix.platform.target, 'x86_64') }}
                    shell: bash
                    run: |
                      set -e
                      uv venv .venv
                      source .venv/bin/activate
                      uv pip install example --no-index --no-deps --find-links dist --reinstall
                      uv pip install example pytest
                      pytest
                  - name: pytest
                    if: ${{ !startsWith(matrix.platform.target, 'x86') && matrix.platform.target != 'ppc64' }}
                    uses: uraimo/run-on-arch-action@v2
                    with:
                      arch: ${{ matrix.platform.target }}
                      distro: ubuntu22.04
                      githubToken: ${{ github.token }}
                      install: |
                        apt-get update
                        apt-get install -y --no-install-recommends python3 python3-pip
                        pip3 install -U pip pytest
                      run: |
                        set -e
                        pip3 install example --no-index --no-deps --find-links dist --force-reinstall
                        pip3 install example
                        pytest

              musllinux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: x86
                      - runner: ubuntu-22.04
                        target: aarch64
                      - runner: ubuntu-22.04
                        target: armv7
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: musllinux_1_2
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-musllinux-${{ matrix.platform.target }}
                      path: dist
                  - name: pytest
                    if: ${{ startsWith(matrix.platform.target, 'x86_64') }}
                    run: |
                      set -e
                      docker run --rm -v ${{ github.workspace }}:/io -w /io alpine:latest sh -c '
                        apk add py3-pip py3-virtualenv
                        python3 -m virtualenv .venv
                        source .venv/bin/activate
                        pip install example --no-index --no-deps --find-links dist --force-reinstall
                        pip install example pytest
                        pytest
                      '
                  - name: pytest
                    if: ${{ !startsWith(matrix.platform.target, 'x86') }}
                    uses: uraimo/run-on-arch-action@v2
                    with:
                      arch: ${{ matrix.platform.target }}
                      distro: alpine_latest
                      githubToken: ${{ github.token }}
                      install: |
                        apk add py3-virtualenv
                      run: |
                        set -e
                        python3 -m virtualenv .venv
                        source .venv/bin/activate
                        pip install example --no-index --no-deps --find-links dist --force-reinstall
                        pip install example pytest
                        pytest

              windows:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: windows-latest
                        target: x64
                        python_arch: x64
                      - runner: windows-latest
                        target: x86
                        python_arch: x86
                      - runner: windows-11-arm
                        target: aarch64
                        python_arch: arm64
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.13
                      architecture: ${{ matrix.platform.python_arch }}
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-windows-${{ matrix.platform.target }}
                      path: dist
                  - uses: astral-sh/setup-uv@v7
                  - name: pytest
                    shell: bash
                    run: |
                      set -e
                      uv venv .venv
                      source .venv/Scripts/activate
                      uv pip install example --no-index --no-deps --find-links dist --reinstall
                      uv pip install example pytest
                      pytest

              macos:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: macos-15-intel
                        target: x86_64
                      - runner: macos-latest
                        target: aarch64
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-macos-${{ matrix.platform.target }}
                      path: dist
                  - uses: astral-sh/setup-uv@v7
                  - name: pytest
                    run: |
                      set -e
                      uv venv .venv
                      source .venv/bin/activate
                      uv pip install example --no-index --no-deps --find-links dist --reinstall
                      uv pip install example pytest
                      pytest

              sdist:
                runs-on: ubuntu-latest
                steps:
                  - uses: actions/checkout@v6
                  - name: Build sdist
                    uses: PyO3/maturin-action@v1
                    with:
                      command: sdist
                      args: --out dist
                  - name: Upload sdist
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-sdist
                      path: dist

              release:
                name: Release
                runs-on: ubuntu-latest
                if: ${{ startsWith(github.ref, 'refs/tags/') || github.event_name == 'workflow_dispatch' }}
                needs: [linux, musllinux, windows, macos, sdist]
                permissions:
                  # Use to sign the release artifacts
                  id-token: write
                  # Used to upload release artifacts
                  contents: write
                  # Used to generate artifact attestation
                  attestations: write
                steps:
                  - uses: actions/download-artifact@v7
                  - name: Generate artifact attestation
                    uses: actions/attest-build-provenance@v3
                    with:
                      subject-path: 'wheels-*/*'
                  - name: Install uv
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    uses: astral-sh/setup-uv@v7
                  - name: Publish to PyPI
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    run: uv publish 'wheels-*/*'
                    env:
                      UV_PUBLISH_TOKEN: ${{ secrets.PYPI_API_TOKEN }}"#]];
        expected.assert_eq(&conf);
    }

    #[test]
    fn test_generate_github_bin_no_binding() {
        let conf = super::generate_github_from_cli(
            &GenerateCI::default(),
            "example",
            &BridgeModel::Bin(None),
            true,
        )
        .unwrap()
        .lines()
        .skip(5)
        .collect::<Vec<_>>()
        .join("\n");
        let expected = expect![[r#"
            name: CI

            on:
              push:
                branches:
                  - main
                  - master
                tags:
                  - '*'
              pull_request:
              workflow_dispatch:

            permissions:
              contents: read

            jobs:
              linux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: x86
                      - runner: ubuntu-22.04
                        target: aarch64
                      - runner: ubuntu-22.04
                        target: armv7
                      - runner: ubuntu-22.04
                        target: s390x
                      - runner: ubuntu-22.04
                        target: ppc64le
                steps:
                  - uses: actions/checkout@v6
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: auto
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-linux-${{ matrix.platform.target }}
                      path: dist

              musllinux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: x86
                      - runner: ubuntu-22.04
                        target: aarch64
                      - runner: ubuntu-22.04
                        target: armv7
                steps:
                  - uses: actions/checkout@v6
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: musllinux_1_2
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-musllinux-${{ matrix.platform.target }}
                      path: dist

              windows:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: windows-latest
                        target: x64
                        python_arch: x64
                      - runner: windows-latest
                        target: x86
                        python_arch: x86
                      - runner: windows-11-arm
                        target: aarch64
                        python_arch: arm64
                steps:
                  - uses: actions/checkout@v6
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-windows-${{ matrix.platform.target }}
                      path: dist

              macos:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: macos-15-intel
                        target: x86_64
                      - runner: macos-latest
                        target: aarch64
                steps:
                  - uses: actions/checkout@v6
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-macos-${{ matrix.platform.target }}
                      path: dist

              sdist:
                runs-on: ubuntu-latest
                steps:
                  - uses: actions/checkout@v6
                  - name: Build sdist
                    uses: PyO3/maturin-action@v1
                    with:
                      command: sdist
                      args: --out dist
                  - name: Upload sdist
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-sdist
                      path: dist

              release:
                name: Release
                runs-on: ubuntu-latest
                if: ${{ startsWith(github.ref, 'refs/tags/') || github.event_name == 'workflow_dispatch' }}
                needs: [linux, musllinux, windows, macos, sdist]
                permissions:
                  # Use to sign the release artifacts
                  id-token: write
                  # Used to upload release artifacts
                  contents: write
                  # Used to generate artifact attestation
                  attestations: write
                steps:
                  - uses: actions/download-artifact@v7
                  - name: Generate artifact attestation
                    uses: actions/attest-build-provenance@v3
                    with:
                      subject-path: 'wheels-*/*'
                  - name: Install uv
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    uses: astral-sh/setup-uv@v7
                  - name: Publish to PyPI
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    run: uv publish 'wheels-*/*'
                    env:
                      UV_PUBLISH_TOKEN: ${{ secrets.PYPI_API_TOKEN }}"#]];
        expected.assert_eq(&conf);
    }

    #[test]
    fn test_generate_github_bin_skips_cli_emscripten() {
        let cli = GenerateCI {
            platforms: vec![Platform::Emscripten],
            ..Default::default()
        };
        let resolved = super::resolve_config(&cli, None, &BridgeModel::Bin(None)).unwrap();

        assert!(
            !resolved
                .platform_targets
                .contains_key(&Platform::Emscripten)
        );
    }

    #[test]
    fn test_generate_github_pyproject_simple_targets() {
        // Test: pyproject specifies only linux and macos with limited targets
        use crate::pyproject_toml::{GitHubCIConfig, PlatformCIConfig};

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

        let cli = GenerateCI::default();
        let bridge = BridgeModel::PyO3(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version: Version::new(0, 23, 0),
            abi3: None,
            metadata: None,
        });
        let resolved = super::resolve_config(&cli, Some(&github_config), &bridge).unwrap();
        let conf = super::generate_github(&cli, &resolved, "example", &bridge, true)
            .unwrap()
            .lines()
            .skip(5)
            .collect::<Vec<_>>()
            .join("\n");
        let expected = expect![[r#"
            name: CI

            on:
              push:
                branches:
                  - main
                  - master
                tags:
                  - '*'
              pull_request:
              workflow_dispatch:

            permissions:
              contents: read

            jobs:
              linux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                      - runner: ubuntu-22.04
                        target: aarch64
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: auto
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-linux-${{ matrix.platform.target }}
                      path: dist

              macos:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: macos-latest
                        target: aarch64
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-macos-${{ matrix.platform.target }}
                      path: dist

              sdist:
                runs-on: ubuntu-latest
                steps:
                  - uses: actions/checkout@v6
                  - name: Build sdist
                    uses: PyO3/maturin-action@v1
                    with:
                      command: sdist
                      args: --out dist
                  - name: Upload sdist
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-sdist
                      path: dist

              release:
                name: Release
                runs-on: ubuntu-latest
                if: ${{ startsWith(github.ref, 'refs/tags/') || github.event_name == 'workflow_dispatch' }}
                needs: [linux, macos, sdist]
                permissions:
                  # Use to sign the release artifacts
                  id-token: write
                  # Used to upload release artifacts
                  contents: write
                  # Used to generate artifact attestation
                  attestations: write
                steps:
                  - uses: actions/download-artifact@v7
                  - name: Generate artifact attestation
                    uses: actions/attest-build-provenance@v3
                    with:
                      subject-path: 'wheels-*/*'
                  - name: Install uv
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    uses: astral-sh/setup-uv@v7
                  - name: Publish to PyPI
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    run: uv publish 'wheels-*/*'
                    env:
                      UV_PUBLISH_TOKEN: ${{ secrets.PYPI_API_TOKEN }}"#]];
        expected.assert_eq(&conf);
    }

    #[test]
    fn test_generate_github_pyproject_detailed_targets() {
        // Test: detailed [[target]] with per-target runner and manylinux overrides
        use crate::pyproject_toml::{GitHubCIConfig, PlatformCIConfig, TargetCIConfig};

        let github_config = GitHubCIConfig {
            pytest: None,
            zig: None,
            skip_attestation: None,
            linux: Some(PlatformCIConfig {
                runner: Some("ubuntu-22.04".to_string()),
                manylinux: Some("2_28".to_string()),
                target: Some(vec![
                    TargetCIConfig {
                        arch: "x86_64".to_string(),
                        runner: None,
                        manylinux: None,
                        container: None,
                        docker_options: None,
                        rust_toolchain: None,
                        rustup_components: None,
                        before_script_linux: None,
                        args: None,
                    },
                    TargetCIConfig {
                        arch: "aarch64".to_string(),
                        runner: Some("self-hosted-arm64".to_string()),
                        manylinux: Some("2_17".to_string()),
                        container: None,
                        docker_options: None,
                        rust_toolchain: None,
                        rustup_components: None,
                        before_script_linux: Some("yum install -y openssl-devel".to_string()),
                        args: None,
                    },
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let cli = GenerateCI::default();
        let bridge = BridgeModel::PyO3(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version: Version::new(0, 23, 0),
            abi3: None,
            metadata: None,
        });
        let resolved = super::resolve_config(&cli, Some(&github_config), &bridge).unwrap();
        let conf = super::generate_github(&cli, &resolved, "example", &bridge, false)
            .unwrap()
            .lines()
            .skip(5)
            .collect::<Vec<_>>()
            .join("\n");
        let expected = expect![[r#"
            name: CI

            on:
              push:
                branches:
                  - main
                  - master
                tags:
                  - '*'
              pull_request:
              workflow_dispatch:

            permissions:
              contents: read

            jobs:
              linux:
                runs-on: ${{ matrix.platform.runner }}
                strategy:
                  matrix:
                    platform:
                      - runner: ubuntu-22.04
                        target: x86_64
                        manylinux: 2_28
                      - runner: self-hosted-arm64
                        target: aarch64
                        manylinux: 2_17
                        before_script_linux: yum install -y openssl-devel
                steps:
                  - uses: actions/checkout@v6
                  - uses: actions/setup-python@v6
                    with:
                      python-version: 3.x
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.platform.target }}
                      args: --release --out dist --find-interpreter
                      sccache: ${{ !startsWith(github.ref, 'refs/tags/') }}
                      manylinux: ${{ matrix.platform.manylinux }}
                      before-script-linux: ${{ matrix.platform.before_script_linux }}
                  - name: Upload wheels
                    uses: actions/upload-artifact@v6
                    with:
                      name: wheels-linux-${{ matrix.platform.target }}
                      path: dist

              release:
                name: Release
                runs-on: ubuntu-latest
                if: ${{ startsWith(github.ref, 'refs/tags/') || github.event_name == 'workflow_dispatch' }}
                needs: [linux]
                permissions:
                  # Use to sign the release artifacts
                  id-token: write
                  # Used to upload release artifacts
                  contents: write
                  # Used to generate artifact attestation
                  attestations: write
                steps:
                  - uses: actions/download-artifact@v7
                  - name: Generate artifact attestation
                    uses: actions/attest-build-provenance@v3
                    with:
                      subject-path: 'wheels-*/*'
                  - name: Install uv
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    uses: astral-sh/setup-uv@v7
                  - name: Publish to PyPI
                    if: ${{ startsWith(github.ref, 'refs/tags/') }}
                    run: uv publish 'wheels-*/*'
                    env:
                      UV_PUBLISH_TOKEN: ${{ secrets.PYPI_API_TOKEN }}"#]];
        expected.assert_eq(&conf);
    }

    #[test]
    fn test_generate_github_pyproject_cli_override() {
        // Test: CLI --platform overrides pyproject platform presence
        use crate::pyproject_toml::{GitHubCIConfig, PlatformCIConfig};

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

        // CLI specifies only windows platform — should override pyproject platform selection
        let cli = GenerateCI {
            platforms: vec![Platform::Windows],
            ..Default::default()
        };
        let bridge = BridgeModel::Bin(None);
        let resolved = super::resolve_config(&cli, Some(&github_config), &bridge).unwrap();

        // Platform should be only windows (from CLI)
        assert!(resolved.platform_targets.contains_key(&Platform::Windows));
        assert!(!resolved.platform_targets.contains_key(&Platform::ManyLinux));
        assert!(!resolved.platform_targets.contains_key(&Platform::Macos));

        // Booleans come from pyproject since CLI didn't set them (CLI flags are false = "not set")
        assert!(resolved.pytest);
        assert!(resolved.zig);
        assert!(resolved.skip_attestation);
    }

    #[test]
    fn test_generate_github_pyproject_booleans_from_config() {
        // Test: booleans from pyproject when CLI doesn't set them
        use crate::pyproject_toml::GitHubCIConfig;

        let github_config = GitHubCIConfig {
            pytest: Some(true),
            zig: Some(true),
            skip_attestation: Some(true),
            ..Default::default()
        };

        let cli = GenerateCI::default();
        let bridge = BridgeModel::Bin(None);
        let resolved = super::resolve_config(&cli, Some(&github_config), &bridge).unwrap();

        assert!(resolved.pytest);
        assert!(resolved.zig);
        assert!(resolved.skip_attestation);
    }

    #[test]
    fn test_generate_github_pyproject_cli_bool_override() {
        // Test: CLI --pytest overrides pyproject pytest=false
        use crate::pyproject_toml::GitHubCIConfig;

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
        let resolved = super::resolve_config(&cli, Some(&github_config), &bridge).unwrap();

        assert!(resolved.pytest);
        assert!(resolved.zig);
        assert!(resolved.skip_attestation);
    }

    #[test]
    fn test_generate_github_pyproject_mutual_exclusion_error() {
        // Test: targets and [[target]] are mutually exclusive
        use crate::pyproject_toml::{GitHubCIConfig, PlatformCIConfig, TargetCIConfig};

        let github_config = GitHubCIConfig {
            linux: Some(PlatformCIConfig {
                targets: Some(vec!["x86_64".to_string()]),
                target: Some(vec![TargetCIConfig {
                    arch: "x86_64".to_string(),
                    runner: None,
                    manylinux: None,
                    container: None,
                    docker_options: None,
                    rust_toolchain: None,
                    rustup_components: None,
                    before_script_linux: None,
                    args: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let cli = GenerateCI::default();
        let bridge = BridgeModel::Bin(None);
        let result = super::resolve_config(&cli, Some(&github_config), &bridge);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("mutually exclusive"),
            "Expected mutual exclusion error, got: {err_msg}"
        );
    }

    #[test]
    fn test_generate_github_pyproject_platform_level_runner() {
        // Test: platform-level runner applies to all targets
        use crate::pyproject_toml::{GitHubCIConfig, PlatformCIConfig};

        let github_config = GitHubCIConfig {
            linux: Some(PlatformCIConfig {
                runner: Some("self-hosted-linux".to_string()),
                targets: Some(vec!["x86_64".to_string(), "aarch64".to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let cli = GenerateCI::default();
        let bridge = BridgeModel::Bin(None);
        let resolved = super::resolve_config(&cli, Some(&github_config), &bridge).unwrap();
        let targets = &resolved.platform_targets[&Platform::ManyLinux];

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].runner, "self-hosted-linux");
        assert_eq!(targets[1].runner, "self-hosted-linux");
    }

    #[test]
    fn test_generate_github_pyproject_uniform_manylinux() {
        // Test: uniform manylinux emitted at step level, not in matrix
        use crate::pyproject_toml::{GitHubCIConfig, PlatformCIConfig};

        let github_config = GitHubCIConfig {
            linux: Some(PlatformCIConfig {
                manylinux: Some("2_28".to_string()),
                targets: Some(vec!["x86_64".to_string(), "aarch64".to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let cli = GenerateCI::default();
        let bridge = BridgeModel::PyO3(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version: Version::new(0, 23, 0),
            abi3: None,
            metadata: None,
        });
        let resolved = super::resolve_config(&cli, Some(&github_config), &bridge).unwrap();
        let conf = super::generate_github(&cli, &resolved, "example", &bridge, false).unwrap();

        // manylinux should appear as step-level `manylinux: 2_28`, not as matrix reference
        assert!(conf.contains("          manylinux: 2_28\n"));
        assert!(!conf.contains("matrix.platform.manylinux"));
    }

    #[test]
    fn test_generate_github_android() {
        use crate::pyproject_toml::{GitHubCIConfig, PlatformCIConfig};

        let github_config = GitHubCIConfig {
            android: Some(PlatformCIConfig::default()),
            ..Default::default()
        };

        let cli = GenerateCI::default();
        let bridge = BridgeModel::PyO3(PyO3 {
            crate_name: PyO3Crate::PyO3,
            version: Version::new(0, 23, 0),
            abi3: None,
            metadata: None,
        });
        let resolved = super::resolve_config(&cli, Some(&github_config), &bridge).unwrap();
        let conf = super::generate_github(&cli, &resolved, "example", &bridge, false).unwrap();

        // Android job should exist
        assert!(conf.contains("  android:\n"));
        // Should use ubuntu-latest runner
        assert!(conf.contains("runner: ubuntu-latest"));
        // Should have default targets matching cibuildwheel
        assert!(conf.contains("target: aarch64-linux-android"));
        assert!(conf.contains("target: x86_64-linux-android"));
        // Should NOT have setup-python
        assert!(!conf.contains("actions/setup-python"));
        // Should NOT have --find-interpreter
        assert!(!conf.contains("--find-interpreter"));
        // Should NOT have manylinux
        assert!(!conf.contains("manylinux:"));
    }

    #[test]
    fn test_generate_github_android_bin() {
        use crate::pyproject_toml::{GitHubCIConfig, PlatformCIConfig};

        let github_config = GitHubCIConfig {
            android: Some(PlatformCIConfig::default()),
            linux: Some(PlatformCIConfig::default()),
            ..Default::default()
        };

        let cli = GenerateCI::default();
        let bridge = BridgeModel::Bin(None);
        let resolved = super::resolve_config(&cli, Some(&github_config), &bridge).unwrap();
        let conf = super::generate_github(&cli, &resolved, "example", &bridge, false).unwrap();

        // Android should NOT be skipped for bin-only projects
        assert!(conf.contains("  android:\n"));
        assert!(conf.contains("  linux:\n"));
    }
}
