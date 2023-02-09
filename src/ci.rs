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
#[derive(Debug, Clone, Copy, ValueEnum)]
#[clap(rename_all = "lower")]
pub enum Platform {
    /// Linux
    Linux,
    /// Windows
    Windows,
    /// macOS
    Macos,
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Platform::Linux => write!(f, "linux"),
            Platform::Windows => write!(f, "windows"),
            Platform::Macos => write!(f, "macos"),
        }
    }
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
        num_args = 0..,
        default_values_t = vec![Platform::Linux, Platform::Windows, Platform::Macos],
    )]
    pub platforms: Vec<Platform>,
    /// Enable pytest
    #[arg(long)]
    pub pytest: bool,
    /// Use zig to do cross compilation
    #[arg(long)]
    pub zig: bool,
}

impl Default for GenerateCI {
    fn default() -> Self {
        Self {
            ci: Provider::GitHub,
            manifest_path: None,
            output: PathBuf::from("-"),
            platforms: vec![Platform::Linux, Platform::Windows, Platform::Macos],
            pytest: false,
            zig: false,
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
        } = ProjectResolver::resolve(self.manifest_path.clone(), cargo_options)?;
        let pyproject = pyproject_toml.as_ref();
        let bridge = find_bridge(&cargo_metadata, pyproject.and_then(|x| x.bindings()))?;
        let project_name = pyproject
            .and_then(|project| project.project_name())
            .unwrap_or(&project_layout.extension_name);

        match self.ci {
            Provider::GitHub => self.generate_github(project_name, &bridge),
        }
    }

    pub(crate) fn generate_github(
        &self,
        project_name: &str,
        bridge_model: &BridgeModel,
    ) -> Result<String> {
        let is_abi3 = matches!(bridge_model, BridgeModel::BindingsAbi3(..));
        let is_bin = bridge_model.is_bin();
        let setup_python = self.pytest
            || matches!(
                bridge_model,
                BridgeModel::Bin(Some(_))
                    | BridgeModel::Bindings(..)
                    | BridgeModel::BindingsAbi3(..)
                    | BridgeModel::Cffi
                    | BridgeModel::UniFfi
            );
        let mut conf = "on:
  push:
    branches:
      - main
      - master
  pull_request:
  workflow_dispatch:

jobs:\n"
            .to_string();

        let mut needs = Vec::new();
        for platform in &self.platforms {
            let plat_name = platform.to_string();
            let os_name = match platform {
                Platform::Linux => "ubuntu",
                _ => &plat_name,
            };
            needs.push(platform.to_string());
            conf.push_str(&format!(
                "  {plat_name}:
    runs-on: {os_name}-latest\n"
            ));
            // target matrix
            let targets = match platform {
                Platform::Linux => vec!["x86_64", "x86", "aarch64", "armv7", "s390x", "ppc64le"],
                Platform::Windows => vec!["x64", "x86"],
                Platform::Macos => vec!["x86_64", "aarch64"],
            };
            conf.push_str(&format!(
                "    strategy:
      matrix:
        target: [{targets}]\n",
                targets = targets.join(", ")
            ));
            // job steps
            conf.push_str(
                "    steps:
      - uses: actions/checkout@v3\n",
            );
            // setup python on demand
            if setup_python {
                conf.push_str(
                    "      - uses: actions/setup-python@v4
        with:
          python-version: '3.10'\n",
                );
                if matches!(platform, Platform::Windows) {
                    conf.push_str("          architecture: ${{ matrix.target }}\n");
                }
            }
            // build wheels
            let mut maturin_args = if is_abi3 || (is_bin && !setup_python) {
                Vec::new()
            } else {
                vec!["--find-interpreter".to_string()]
            };
            if let Some(manifest_path) = self.manifest_path.as_ref() {
                if manifest_path != Path::new("Cargo.toml") {
                    maturin_args.push("--manifest-path".to_string());
                    maturin_args.push(manifest_path.display().to_string())
                }
            }
            if self.zig && matches!(platform, Platform::Linux) {
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
          target: ${{{{ matrix.target }}}}
          args: --release --out dist{maturin_args}
"
            ));
            if matches!(platform, Platform::Linux) {
                conf.push_str("          manylinux: auto\n");
            }
            // upload wheels
            conf.push_str(
                "      - name: Upload wheels
        uses: actions/upload-artifact@v3
        with:
          name: wheels
          path: dist
",
            );
            // pytest
            let mut chdir = String::new();
            if let Some(manifest_path) = self.manifest_path.as_ref() {
                if manifest_path != Path::new("Cargo.toml") {
                    let parent = manifest_path.parent().unwrap();
                    chdir = format!("cd {} && ", parent.display());
                }
            }
            if self.pytest {
                if matches!(platform, Platform::Linux) {
                    // Test on host for x86_64
                    conf.push_str(&format!(
                        "      - name: pytest
        if: ${{{{ startsWith(matrix.target, 'x86_64') }}}}
        shell: bash
        run: |
          set -e
          pip install {project_name} --find-links dist --force-reinstall
          pip install pytest
          {chdir}pytest
"
                    ));
                    // Test on QEMU for other architectures
                    conf.push_str(&format!(
                        "      - name: pytest
        if: ${{{{ !startsWith(matrix.target, 'x86') && matrix.target != 'ppc64' }}}}
        uses: uraimo/run-on-arch-action@v2.5.0
        with:
          arch: ${{{{ matrix.target }}}}
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
                } else {
                    conf.push_str(&format!(
                        "      - name: pytest
        if: ${{{{ !startsWith(matrix.target, 'aarch64') }}}}
        shell: bash
        run: |
          set -e
          pip install {project_name} --find-links dist --force-reinstall
          pip install pytest
          {chdir}pytest
"
                    ));
                }
            }

            conf.push('\n');
        }

        conf.push_str(&format!(
            r#"  release:
    name: Release
    runs-on: ubuntu-latest
    if: "startsWith(github.ref, 'refs/tags/')"
    needs: [{needs}]
    steps:
      - uses: actions/download-artifact@v3
        with:
          name: wheels
      - name: Publish to PyPI
        uses: PyO3/maturin-action@v1
        env:
          MATURIN_PYPI_TOKEN: ${{{{ secrets.PYPI_API_TOKEN }}}}
        with:
          command: upload
          args: --skip-existing *
"#,
            needs = needs.join(", ")
        ));
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
    use crate::BridgeModel;
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_generate_github() {
        let conf = GenerateCI::default()
            .generate_github("example", &BridgeModel::Bindings("pyo3".to_string(), 7))
            .unwrap();
        let expected = indoc! {r#"
            on:
              push:
                branches:
                  - main
                  - master
              pull_request:
              workflow_dispatch:

            jobs:
              linux:
                runs-on: ubuntu-latest
                strategy:
                  matrix:
                    target: [x86_64, x86, aarch64, armv7, s390x, ppc64le]
                steps:
                  - uses: actions/checkout@v3
                  - uses: actions/setup-python@v4
                    with:
                      python-version: '3.10'
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist --find-interpreter
                      manylinux: auto
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist

              windows:
                runs-on: windows-latest
                strategy:
                  matrix:
                    target: [x64, x86]
                steps:
                  - uses: actions/checkout@v3
                  - uses: actions/setup-python@v4
                    with:
                      python-version: '3.10'
                      architecture: ${{ matrix.target }}
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist --find-interpreter
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist

              macos:
                runs-on: macos-latest
                strategy:
                  matrix:
                    target: [x86_64, aarch64]
                steps:
                  - uses: actions/checkout@v3
                  - uses: actions/setup-python@v4
                    with:
                      python-version: '3.10'
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist --find-interpreter
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist

              release:
                name: Release
                runs-on: ubuntu-latest
                if: "startsWith(github.ref, 'refs/tags/')"
                needs: [linux, windows, macos]
                steps:
                  - uses: actions/download-artifact@v3
                    with:
                      name: wheels
                  - name: Publish to PyPI
                    uses: PyO3/maturin-action@v1
                    env:
                      MATURIN_PYPI_TOKEN: ${{ secrets.PYPI_API_TOKEN }}
                    with:
                      command: upload
                      args: --skip-existing *
        "#};
        assert_eq!(conf, expected);
    }

    #[test]
    fn test_generate_github_abi3() {
        let conf = GenerateCI::default()
            .generate_github("example", &BridgeModel::BindingsAbi3(3, 7))
            .unwrap();
        let expected = indoc! {r#"
            on:
              push:
                branches:
                  - main
                  - master
              pull_request:
              workflow_dispatch:

            jobs:
              linux:
                runs-on: ubuntu-latest
                strategy:
                  matrix:
                    target: [x86_64, x86, aarch64, armv7, s390x, ppc64le]
                steps:
                  - uses: actions/checkout@v3
                  - uses: actions/setup-python@v4
                    with:
                      python-version: '3.10'
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist
                      manylinux: auto
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist

              windows:
                runs-on: windows-latest
                strategy:
                  matrix:
                    target: [x64, x86]
                steps:
                  - uses: actions/checkout@v3
                  - uses: actions/setup-python@v4
                    with:
                      python-version: '3.10'
                      architecture: ${{ matrix.target }}
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist

              macos:
                runs-on: macos-latest
                strategy:
                  matrix:
                    target: [x86_64, aarch64]
                steps:
                  - uses: actions/checkout@v3
                  - uses: actions/setup-python@v4
                    with:
                      python-version: '3.10'
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist

              release:
                name: Release
                runs-on: ubuntu-latest
                if: "startsWith(github.ref, 'refs/tags/')"
                needs: [linux, windows, macos]
                steps:
                  - uses: actions/download-artifact@v3
                    with:
                      name: wheels
                  - name: Publish to PyPI
                    uses: PyO3/maturin-action@v1
                    env:
                      MATURIN_PYPI_TOKEN: ${{ secrets.PYPI_API_TOKEN }}
                    with:
                      command: upload
                      args: --skip-existing *
        "#};
        assert_eq!(conf, expected);
    }

    #[test]
    fn test_generate_github_zig_pytest() {
        let gen = GenerateCI {
            zig: true,
            pytest: true,
            ..Default::default()
        };
        let conf = gen
            .generate_github("example", &BridgeModel::Bindings("pyo3".to_string(), 7))
            .unwrap();
        let expected = indoc! {r#"
            on:
              push:
                branches:
                  - main
                  - master
              pull_request:
              workflow_dispatch:

            jobs:
              linux:
                runs-on: ubuntu-latest
                strategy:
                  matrix:
                    target: [x86_64, x86, aarch64, armv7, s390x, ppc64le]
                steps:
                  - uses: actions/checkout@v3
                  - uses: actions/setup-python@v4
                    with:
                      python-version: '3.10'
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist --find-interpreter --zig
                      manylinux: auto
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist
                  - name: pytest
                    if: ${{ startsWith(matrix.target, 'x86_64') }}
                    shell: bash
                    run: |
                      set -e
                      pip install example --find-links dist --force-reinstall
                      pip install pytest
                      pytest
                  - name: pytest
                    if: ${{ !startsWith(matrix.target, 'x86') && matrix.target != 'ppc64' }}
                    uses: uraimo/run-on-arch-action@v2.5.0
                    with:
                      arch: ${{ matrix.target }}
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

              windows:
                runs-on: windows-latest
                strategy:
                  matrix:
                    target: [x64, x86]
                steps:
                  - uses: actions/checkout@v3
                  - uses: actions/setup-python@v4
                    with:
                      python-version: '3.10'
                      architecture: ${{ matrix.target }}
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist --find-interpreter
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist
                  - name: pytest
                    if: ${{ !startsWith(matrix.target, 'aarch64') }}
                    shell: bash
                    run: |
                      set -e
                      pip install example --find-links dist --force-reinstall
                      pip install pytest
                      pytest

              macos:
                runs-on: macos-latest
                strategy:
                  matrix:
                    target: [x86_64, aarch64]
                steps:
                  - uses: actions/checkout@v3
                  - uses: actions/setup-python@v4
                    with:
                      python-version: '3.10'
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist --find-interpreter
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist
                  - name: pytest
                    if: ${{ !startsWith(matrix.target, 'aarch64') }}
                    shell: bash
                    run: |
                      set -e
                      pip install example --find-links dist --force-reinstall
                      pip install pytest
                      pytest

              release:
                name: Release
                runs-on: ubuntu-latest
                if: "startsWith(github.ref, 'refs/tags/')"
                needs: [linux, windows, macos]
                steps:
                  - uses: actions/download-artifact@v3
                    with:
                      name: wheels
                  - name: Publish to PyPI
                    uses: PyO3/maturin-action@v1
                    env:
                      MATURIN_PYPI_TOKEN: ${{ secrets.PYPI_API_TOKEN }}
                    with:
                      command: upload
                      args: --skip-existing *
        "#};
        assert_eq!(conf, expected);
    }

    #[test]
    fn test_generate_github_bin_no_binding() {
        let conf = GenerateCI::default()
            .generate_github("example", &BridgeModel::Bin(None))
            .unwrap();
        let expected = indoc! {r#"
            on:
              push:
                branches:
                  - main
                  - master
              pull_request:
              workflow_dispatch:

            jobs:
              linux:
                runs-on: ubuntu-latest
                strategy:
                  matrix:
                    target: [x86_64, x86, aarch64, armv7, s390x, ppc64le]
                steps:
                  - uses: actions/checkout@v3
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist
                      manylinux: auto
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist

              windows:
                runs-on: windows-latest
                strategy:
                  matrix:
                    target: [x64, x86]
                steps:
                  - uses: actions/checkout@v3
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist

              macos:
                runs-on: macos-latest
                strategy:
                  matrix:
                    target: [x86_64, aarch64]
                steps:
                  - uses: actions/checkout@v3
                  - name: Build wheels
                    uses: PyO3/maturin-action@v1
                    with:
                      target: ${{ matrix.target }}
                      args: --release --out dist
                  - name: Upload wheels
                    uses: actions/upload-artifact@v3
                    with:
                      name: wheels
                      path: dist

              release:
                name: Release
                runs-on: ubuntu-latest
                if: "startsWith(github.ref, 'refs/tags/')"
                needs: [linux, windows, macos]
                steps:
                  - uses: actions/download-artifact@v3
                    with:
                      name: wheels
                  - name: Publish to PyPI
                    uses: PyO3/maturin-action@v1
                    env:
                      MATURIN_PYPI_TOKEN: ${{ secrets.PYPI_API_TOKEN }}
                    with:
                      command: upload
                      args: --skip-existing *
        "#};
        assert_eq!(conf, expected);
    }
}
