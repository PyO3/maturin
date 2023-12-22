use crate::build_options::CargoOptions;
use crate::target::Arch;
use crate::BuildOptions;
use crate::PlatformTag;
use crate::PythonInterpreter;
use crate::Target;
use anyhow::{anyhow, bail, Context, Result};
use cargo_options::heading;
use pep508_rs::{MarkerExpression, MarkerOperator, MarkerTree, MarkerValue};
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// Install the crate as module in the current virtualenv
#[derive(Debug, clap::Parser)]
pub struct DevelopOptions {
    /// Which kind of bindings to use
    #[arg(
        short = 'b',
        long = "bindings",
        alias = "binding-crate",
        value_parser = ["pyo3", "pyo3-ffi", "rust-cpython", "cffi", "uniffi", "bin"]
    )]
    pub bindings: Option<String>,
    /// Pass --release to cargo
    #[arg(short = 'r', long, help_heading = heading::COMPILATION_OPTIONS,)]
    pub release: bool,
    /// Strip the library for minimum file size
    #[arg(long)]
    pub strip: bool,
    /// Install extra requires aka. optional dependencies
    ///
    /// Use as `--extras=extra1,extra2`
    #[arg(
        short = 'E',
        long,
        value_delimiter = ',',
        action = clap::ArgAction::Append
    )]
    pub extras: Vec<String>,
    /// Skip installation, only build the extension module inplace
    ///
    /// Only works with mixed Rust/Python project layout
    #[arg(long)]
    pub skip_install: bool,
    /// Use a specific pip installation instead of the default one.
    ///
    /// This can be used to supply the path to a pip executable when the
    /// current virtualenv does not provide one.
    #[arg(long)]
    pub pip_path: Option<PathBuf>,
    /// `cargo rustc` options
    #[command(flatten)]
    pub cargo_options: CargoOptions,
}

fn make_pip_command(python_path: &Path, pip_path: Option<&Path>) -> Command {
    match pip_path {
        Some(pip_path) => {
            let mut cmd = Command::new(pip_path);
            cmd.arg("--python")
                .arg(python_path)
                .arg("--disable-pip-version-check");
            cmd
        }
        None => {
            let mut cmd = Command::new(python_path);
            cmd.arg("-m").arg("pip").arg("--disable-pip-version-check");
            cmd
        }
    }
}

/// Installs a crate by compiling it and copying the shared library to site-packages.
/// Also adds the dist-info directory to make sure pip and other tools detect the library
///
/// Works only in a virtualenv.
#[allow(clippy::too_many_arguments)]
pub fn develop(develop_options: DevelopOptions, venv_dir: &Path) -> Result<()> {
    let DevelopOptions {
        bindings,
        release,
        strip,
        extras,
        skip_install,
        pip_path,
        cargo_options,
    } = develop_options;
    let mut target_triple = cargo_options.target.as_ref().map(|x| x.to_string());
    let target = Target::from_target_triple(cargo_options.target)?;
    let python = target.get_venv_python(venv_dir);

    // check python platform and architecture
    if !target.user_specified {
        match Command::new(&python)
            .arg("-c")
            .arg("import sysconfig; print(sysconfig.get_platform(), end='')")
            .output()
        {
            Ok(output) if output.status.success() => {
                let platform = String::from_utf8_lossy(&output.stdout);
                if platform.contains("macos") {
                    if platform.contains("x86_64") && target.target_arch() != Arch::X86_64 {
                        target_triple = Some("x86_64-apple-darwin".to_string());
                    } else if platform.contains("arm64") && target.target_arch() != Arch::Aarch64 {
                        target_triple = Some("aarch64-apple-darwin".to_string());
                    }
                }
            }
            _ => eprintln!("⚠️  Warning: Failed to determine python platform"),
        }
    }

    // Store wheel in a unique location so we don't get name clashes with parallel runs
    let wheel_dir = TempDir::new().context("Failed to create temporary directory")?;

    let build_options = BuildOptions {
        platform_tag: vec![PlatformTag::Linux],
        interpreter: vec![python.clone()],
        find_interpreter: false,
        bindings,
        out: Some(wheel_dir.path().to_path_buf()),
        skip_auditwheel: false,
        #[cfg(feature = "zig")]
        zig: false,
        cargo: CargoOptions {
            target: target_triple,
            ..cargo_options
        },
    };

    let build_context = build_options.into_build_context(release, strip, true)?;

    let interpreter =
        PythonInterpreter::check_executable(&python, &target, build_context.bridge())?.ok_or_else(
            || anyhow!("Expected `python` to be a python interpreter inside a virtualenv ಠ_ಠ"),
        )?;

    // Install dependencies
    if !build_context.metadata21.requires_dist.is_empty() {
        let mut args = vec!["install".to_string()];
        args.extend(build_context.metadata21.requires_dist.iter().map(|x| {
            let mut pkg = x.clone();
            // Remove extra marker to make it installable with pip
            // Keep in sync with `Metadata21::merge_pyproject_toml()`!
            for extra in &extras {
                pkg.marker = pkg.marker.and_then(|marker| -> Option<MarkerTree> {
                    match marker.clone() {
                        MarkerTree::Expression(MarkerExpression {
                            l_value: MarkerValue::Extra,
                            operator: MarkerOperator::Equal,
                            r_value: MarkerValue::QuotedString(extra_value),
                        }) if &extra_value == extra => None,
                        MarkerTree::And(and) => match &*and {
                            [existing, MarkerTree::Expression(MarkerExpression {
                                l_value: MarkerValue::Extra,
                                operator: MarkerOperator::Equal,
                                r_value: MarkerValue::QuotedString(extra_value),
                            })] if extra_value == extra => Some(existing.clone()),
                            _ => Some(marker),
                        },
                        _ => Some(marker),
                    }
                });
            }
            pkg.to_string()
        }));
        let status = make_pip_command(&interpreter.executable, pip_path.as_deref())
            .args(&args)
            .status()
            .context("Failed to run pip install")?;
        if !status.success() {
            bail!(r#"pip install finished with "{}""#, status)
        }
    }

    let wheels = build_context.build_wheels()?;
    if !skip_install {
        for (filename, _supported_version) in wheels.iter() {
            let mut pip_cmd = make_pip_command(&python, pip_path.as_deref());
            let output = pip_cmd
                .args(["install", "--no-deps", "--force-reinstall"])
                .arg(dunce::simplified(filename))
                .output()
                .context(format!(
                    "pip install failed (ran {:?} with {:?})",
                    pip_cmd.get_program(),
                    &pip_cmd.get_args().collect::<Vec<_>>(),
                ))?;
            if !output.status.success() {
                bail!(
                "pip install in {} failed running {:?}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\n",
                venv_dir.display(),
                &pip_cmd.get_args().collect::<Vec<_>>(),
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim(),
            );
            }
            if !output.stderr.is_empty() {
                eprintln!(
                    "⚠️ Warning: pip raised a warning running {:?}:\n{}",
                    &pip_cmd.get_args().collect::<Vec<_>>(),
                    String::from_utf8_lossy(&output.stderr).trim(),
                );
            }
            eprintln!(
                "🛠 Installed {}-{}",
                build_context.metadata21.name, build_context.metadata21.version
            );
        }
    }

    Ok(())
}
