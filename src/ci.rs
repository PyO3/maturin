use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{ArgAction, Parser, ValueEnum};
use fs_err as fs;

use crate::build_options::find_bridge;
use crate::project_layout::ProjectResolver;
use crate::{BridgeModel, CargoOptions};

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
}

impl Platform {
    fn defaults() -> Vec<Self> {
        vec![
            Platform::ManyLinux,
            Platform::Musllinux,
            Platform::Windows,
            Platform::Macos,
        ]
    }

    fn all() -> Vec<Self> {
        vec![
            Platform::ManyLinux,
            Platform::Musllinux,
            Platform::Windows,
            Platform::Macos,
            Platform::Emscripten,
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
        }
    }
}

struct MatrixPlatform {
    runner: &'static str,
    target: &'static str,
    python_arch: Option<&'static str>,
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
    /// Platform support
    #[arg(
        id = "platform",
        long,
        action = ArgAction::Append,
        num_args = 1..,
        default_values_t = vec![
            Platform::ManyLinux,
            Platform::Musllinux,
            Platform::Windows,
            Platform::Macos,
        ],
    )]
    pub platforms: Vec<Platform>,
    /// Enable pytest
    #[arg(long)]
    pub pytest: bool,
    /// Use zig to do cross compilation
    #[arg(long)]
    pub zig: bool,
    /// Skip artifact attestation
    #[arg(long)]
    pub skip_attestation: bool,
}

impl Default for GenerateCI {
    fn default() -> Self {
        Self {
            ci: Provider::GitHub,
            manifest_path: None,
            output: PathBuf::from("-"),
            platforms: vec![
                Platform::ManyLinux,
                Platform::Musllinux,
                Platform::Windows,
                Platform::Macos,
            ],
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
        let cargo_options = CargoOptions {
            manifest_path: self.manifest_path.clone(),
            ..Default::default()
        };
        let ProjectResolver {
            cargo_metadata,
            pyproject_toml,
            project_layout,
            ..
        } = ProjectResolver::resolve(self.manifest_path.clone(), cargo_options, false)?;
        let pyproject = pyproject_toml.as_ref();
        let bridge = find_bridge(&cargo_metadata, pyproject.and_then(|x| x.bindings()))?;
        let project_name = pyproject
            .and_then(|project| project.project_name())
            .unwrap_or(&project_layout.extension_name);
        let sdist = pyproject_toml.is_some();

        match self.ci {
            Provider::GitHub => self.generate_github(project_name, &bridge, sdist),
        }
    }

    pub(crate) fn generate_github(
        &self,
        project_name: &str,
        bridge_model: &BridgeModel,
        sdist: bool,
    ) -> Result<String> {
        let is_abi3 = bridge_model.is_abi3();
        let is_bin = bridge_model.is_bin();
        let setup_python = self.pytest
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
        let platforms: BTreeSet<_> = self
            .platforms
            .iter()
            .flat_map(|p| {
                if matches!(p, Platform::All) {
                    if !bridge_model.is_bin() {
                        Platform::all()
                    } else {
                        Platform::defaults()
                    }
                } else {
                    vec![*p]
                }
            })
            .collect();
        for platform in &platforms {
            if bridge_model.is_bin() && matches!(platform, Platform::Emscripten) {
                continue;
            }
            let plat_name = platform.to_string();
            needs.push(plat_name.clone());
            conf.push_str(&format!(
                "  {plat_name}:
    runs-on: ${{{{ matrix.platform.runner }}}}\n"
            ));
            // target matrix
            let targets: Vec<_> = match platform {
                Platform::ManyLinux => ["x86_64", "x86", "aarch64", "armv7", "s390x", "ppc64le"]
                    .into_iter()
                    .map(|target| MatrixPlatform {
                        runner: "ubuntu-22.04",
                        target,
                        python_arch: None,
                    })
                    .collect(),
                Platform::Musllinux => ["x86_64", "x86", "aarch64", "armv7"]
                    .into_iter()
                    .map(|target| MatrixPlatform {
                        runner: "ubuntu-22.04",
                        target,
                        python_arch: None,
                    })
                    .collect(),
                Platform::Windows => ["x64", "x86", "aarch64"]
                    .into_iter()
                    .map(|target| MatrixPlatform {
                        runner: if target == "aarch64" {
                            "windows-11-arm"
                        } else {
                            "windows-latest"
                        },
                        target,
                        python_arch: if target == "aarch64" {
                            Some("arm64")
                        } else {
                            Some(target)
                        },
                    })
                    .collect(),
                Platform::Macos => {
                    vec![
                        MatrixPlatform {
                            runner: "macos-15-intel",
                            target: "x86_64",
                            python_arch: None,
                        },
                        MatrixPlatform {
                            runner: "macos-latest",
                            target: "aarch64",
                            python_arch: None,
                        },
                    ]
                }
                Platform::Emscripten => vec![MatrixPlatform {
                    runner: "ubuntu-22.04",
                    target: "wasm32-unknown-emscripten",
                    python_arch: None,
                }],
                _ => Vec::new(),
            };
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
                if let Some(python_arch) = target.python_arch {
                    conf.push_str(&format!("            python_arch: {}\n", python_arch));
                }
            }
            // job steps
            conf.push_str(
                "    steps:
      - uses: actions/checkout@v6\n",
            );

            // install pyodide-build for emscripten
            if matches!(platform, Platform::Emscripten) {
                // install stable pyodide-build
                conf.push_str("      - run: pip install pyodide-build\n");
                // get the current python version for the installed pyodide-build
                conf.push_str(
                    "      - name: Get Emscripten and Python version info
        shell: bash
        run: |
          echo EMSCRIPTEN_VERSION=$(pyodide config get emscripten_version) >> $GITHUB_ENV
          echo PYTHON_VERSION=$(pyodide config get python_version | cut -d '.' -f 1-2) >> $GITHUB_ENV
          pip uninstall -y pyodide-build\n",
                );
                conf.push_str(
                    "      - uses: mymindstorm/setup-emsdk@v12
        with:
          version: ${{ env.EMSCRIPTEN_VERSION }}
          actions-cache-folder: emsdk-cache\n",
                );
                conf.push_str(
                    "      - uses: actions/setup-python@v6
        with:
          python-version: ${{ env.PYTHON_VERSION }}\n",
                );
                // install pyodide-build again in the right Python version
                conf.push_str("      - run: pip install pyodide-build\n");
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
                        conf.push_str(
                            "          architecture: ${{ matrix.platform.python_arch }}\n",
                        );
                    }
                }
            }

            // build wheels
            let mut maturin_args = if is_abi3 || (is_bin && !setup_python) {
                Vec::new()
            } else if matches!(platform, Platform::Emscripten) {
                vec!["-i".to_string(), "${{ env.PYTHON_VERSION }}".to_string()]
            } else {
                vec!["--find-interpreter".to_string()]
            };
            if let Some(manifest_path) = self.manifest_path.as_ref() {
                if manifest_path != Path::new("Cargo.toml") {
                    maturin_args.push("--manifest-path".to_string());
                    maturin_args.push(manifest_path.display().to_string())
                }
            }
            if self.zig && matches!(platform, Platform::ManyLinux) {
                maturin_args.push("--zig".to_string());
            }
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
          args: --release --out dist{maturin_args}
          sccache: ${{{{ !startsWith(github.ref, 'refs/tags/') }}}}
"
            ));
            let maturin_action_args = match platform {
                Platform::ManyLinux => "manylinux: auto",
                Platform::Musllinux => "manylinux: musllinux_1_2",
                Platform::Emscripten => "rust-toolchain: nightly",
                _ => "",
            };
            if !maturin_action_args.is_empty() {
                conf.push_str(&format!("          {maturin_action_args}\n"));
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
          args: --release --out dist{maturin_args} -i python3.14t
          sccache: ${{{{ !startsWith(github.ref, 'refs/tags/') }}}}
"
                ));
                if !maturin_action_args.is_empty() {
                    conf.push_str(&format!("          {maturin_action_args}\n"));
                }
            }
            // upload wheels
            let artifact_name = match platform {
                Platform::Emscripten => "wasm-wheels".to_string(),
                _ => format!("wheels-{platform}-${{{{ matrix.platform.target }}}}"),
            };
            conf.push_str(&format!(
                "      - name: Upload wheels
        uses: actions/upload-artifact@v5
        with:
          name: {artifact_name}
          path: dist
"
            ));
            // pytest
            let mut chdir = String::new();
            if let Some(manifest_path) = self.manifest_path.as_ref() {
                if manifest_path != Path::new("Cargo.toml") {
                    let parent = manifest_path.parent().unwrap();
                    chdir = format!("cd {} && ", parent.display());
                }
            }
            if self.pytest {
                match platform {
                    Platform::All => {}
                    Platform::ManyLinux => {
                        // Test on host for x86_64 GNU targets
                        conf.push_str(&format!(
                            "      - name: pytest
        if: ${{{{ startsWith(matrix.platform.target, 'x86_64') }}}}
        shell: bash
        run: |
          set -e
          python3 -m venv .venv
          source .venv/bin/activate
          pip install {project_name} --find-links dist --force-reinstall
          pip install pytest
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
            pip3 install {project_name} --find-links dist --force-reinstall
            {chdir}pytest
"
                                            ));
                    }
                    Platform::Musllinux => {
                        conf.push_str(&format!(
                            "      - name: pytest
        if: ${{{{ startsWith(matrix.platform.target, 'x86_64') }}}}
        uses: addnab/docker-run-action@v3
        with:
          image: alpine:latest
          options: -v ${{{{ github.workspace }}}}:/io -w /io
          run: |
            set -e
            apk add py3-pip py3-virtualenv
            python3 -m virtualenv .venv
            source .venv/bin/activate
            pip install {project_name} --no-index --find-links dist --force-reinstall
            pip install pytest
            {chdir}pytest
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
            pip install pytest
            pip install {project_name} --find-links dist --force-reinstall
            {chdir}pytest
"
                        ));
                    }
                    Platform::Windows => {
                        conf.push_str(&format!(
                            "      - name: pytest
        shell: bash
        run: |
          set -e
          python3 -m venv .venv
          source .venv/Scripts/activate
          pip install {project_name} --find-links dist --force-reinstall
          pip install pytest
          {chdir}pytest
"
                        ));
                    }
                    Platform::Macos => {
                        conf.push_str(&format!(
                            "      - name: pytest
        run: |
          set -e
          python3 -m venv .venv
          source .venv/bin/activate
          pip install {project_name} --find-links dist --force-reinstall
          pip install pytest
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
          pip install {project_name} --find-links dist --force-reinstall
          pip install pytest
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

            let maturin_args = self
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
        uses: actions/upload-artifact@v5
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
        if !self.skip_attestation {
            conf.push_str(
                r#"      # Used to generate artifact attestation
      attestations: write
"#,
            );
        }
        conf.push_str(
            r#"    steps:
      - uses: actions/download-artifact@v6
"#,
        );
        if !self.skip_attestation {
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
        if platforms.contains(&Platform::Emscripten) {
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

    fn print(&self, conf: &str) -> Result<()> {
        if self.output == Path::new("-") {
            print!("{conf}");
        } else {
            fs::write(&self.output, conf)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::GenerateCI;
    use crate::{Abi3Version, BridgeModel, PyO3, bridge::PyO3Crate};
    use expect_test::expect;
    use semver::Version;

    #[test]
    fn test_generate_github() {
        let conf = GenerateCI::default()
            .generate_github(
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                  - uses: actions/download-artifact@v6
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
        let conf = GenerateCI::default()
            .generate_github(
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                  - uses: actions/download-artifact@v6
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
        let conf = GenerateCI {
            skip_attestation: true,
            ..Default::default()
        }
        .generate_github(
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                  - uses: actions/download-artifact@v6
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
        let conf = r#gen
            .generate_github(
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
                    uses: actions/upload-artifact@v5
                    with:
                      name: wheels-linux-${{ matrix.platform.target }}
                      path: dist
                  - name: pytest
                    if: ${{ startsWith(matrix.platform.target, 'x86_64') }}
                    shell: bash
                    run: |
                      set -e
                      python3 -m venv .venv
                      source .venv/bin/activate
                      pip install example --find-links dist --force-reinstall
                      pip install pytest
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
                        pip3 install example --find-links dist --force-reinstall
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
                    uses: actions/upload-artifact@v5
                    with:
                      name: wheels-musllinux-${{ matrix.platform.target }}
                      path: dist
                  - name: pytest
                    if: ${{ startsWith(matrix.platform.target, 'x86_64') }}
                    uses: addnab/docker-run-action@v3
                    with:
                      image: alpine:latest
                      options: -v ${{ github.workspace }}:/io -w /io
                      run: |
                        set -e
                        apk add py3-pip py3-virtualenv
                        python3 -m virtualenv .venv
                        source .venv/bin/activate
                        pip install example --no-index --find-links dist --force-reinstall
                        pip install pytest
                        pytest
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
                        pip install pytest
                        pip install example --find-links dist --force-reinstall
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
                    uses: actions/upload-artifact@v5
                    with:
                      name: wheels-windows-${{ matrix.platform.target }}
                      path: dist
                  - name: pytest
                    shell: bash
                    run: |
                      set -e
                      python3 -m venv .venv
                      source .venv/Scripts/activate
                      pip install example --find-links dist --force-reinstall
                      pip install pytest
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
                    uses: actions/upload-artifact@v5
                    with:
                      name: wheels-macos-${{ matrix.platform.target }}
                      path: dist
                  - name: pytest
                    run: |
                      set -e
                      python3 -m venv .venv
                      source .venv/bin/activate
                      pip install example --find-links dist --force-reinstall
                      pip install pytest
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
                    uses: actions/upload-artifact@v5
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
                  - uses: actions/download-artifact@v6
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
        let conf = GenerateCI::default()
            .generate_github("example", &BridgeModel::Bin(None), true)
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                    uses: actions/upload-artifact@v5
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
                  - uses: actions/download-artifact@v6
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
}
