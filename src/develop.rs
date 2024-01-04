use crate::build_options::CargoOptions;
use crate::target::Arch;
use crate::BuildContext;
use crate::BuildOptions;
use crate::PlatformTag;
use crate::PythonInterpreter;
use crate::Target;
use anyhow::{anyhow, bail, Context, Result};
use cargo_options::heading;
use pep508_rs::{MarkerExpression, MarkerOperator, MarkerTree, MarkerValue};
use regex::Regex;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;
use url::Url;

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

fn install_dependencies(
    build_context: &BuildContext,
    extras: &[String],
    interpreter: &PythonInterpreter,
    pip_path: Option<&Path>,
) -> Result<()> {
    if !build_context.metadata21.requires_dist.is_empty() {
        let mut args = vec!["install".to_string()];
        args.extend(build_context.metadata21.requires_dist.iter().map(|x| {
            let mut pkg = x.clone();
            // Remove extra marker to make it installable with pip
            // Keep in sync with `Metadata21::merge_pyproject_toml()`!
            for extra in extras {
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
        let status = make_pip_command(&interpreter.executable, pip_path)
            .args(&args)
            .status()
            .context("Failed to run pip install")?;
        if !status.success() {
            bail!(r#"pip install finished with "{}""#, status)
        }
    }
    Ok(())
}

fn pip_install_wheel(
    build_context: &BuildContext,
    python: &Path,
    venv_dir: &Path,
    pip_path: Option<&Path>,
    wheel_filename: &Path,
) -> Result<()> {
    let mut pip_cmd = make_pip_command(python, pip_path);
    let output = pip_cmd
        .args(["install", "--no-deps", "--force-reinstall"])
        .arg(dunce::simplified(wheel_filename))
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
    fix_direct_url(build_context, python, pip_path)?;
    Ok(())
}

/// Each editable-installed python package has a direct_url.json file that includes a file:// URL
/// indicating the location of the source code of that project. The maturin import hook uses this
/// URL to locate and rebuild editable-installed projects.
///
/// When a maturin package is installed using `pip install -e`, pip takes care of writing the
/// correct URL, however when a maturin package is installed with `maturin develop`, the URL is
/// set to the path to the temporary wheel file created during installation.
fn fix_direct_url(
    build_context: &BuildContext,
    python: &Path,
    pip_path: Option<&Path>,
) -> Result<()> {
    println!("✏️  Setting installed package as editable");
    let mut pip_cmd = make_pip_command(python, pip_path);
    let output = pip_cmd
        .args(["show", "--files"])
        .arg(&build_context.metadata21.name)
        .output()
        .context(format!(
            "pip show failed (ran {:?} with {:?})",
            pip_cmd.get_program(),
            &pip_cmd.get_args().collect::<Vec<_>>(),
        ))?;
    if let Some(direct_url_path) = parse_direct_url_path(&String::from_utf8_lossy(&output.stdout))?
    {
        let project_dir = build_context
            .pyproject_toml_path
            .parent()
            .ok_or_else(|| anyhow!("failed to get project directory"))?;
        let uri = Url::from_file_path(project_dir)
            .map_err(|_| anyhow!("failed to convert project directory to file URL"))?;
        let content = format!("{{\"dir_info\": {{\"editable\": true}}, \"url\": \"{uri}\"}}");
        fs::write(direct_url_path, content)?;
    }
    Ok(())
}

fn parse_direct_url_path(pip_show_output: &str) -> Result<Option<PathBuf>> {
    if let Some(Some(location)) = Regex::new(r"Location: (.*)")?
        .captures(pip_show_output)
        .map(|c| c.get(1))
    {
        if let Some(Some(direct_url_path)) = Regex::new(r"  (.*direct_url.json)")?
            .captures(pip_show_output)
            .map(|c| c.get(1))
        {
            let absolute_path = PathBuf::from(location.as_str()).join(direct_url_path.as_str());
            return Ok(Some(absolute_path));
        }
    }
    Ok(None)
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

    install_dependencies(&build_context, &extras, &interpreter, pip_path.as_deref())?;

    let wheels = build_context.build_wheels()?;
    if !skip_install {
        for (filename, _supported_version) in wheels.iter() {
            pip_install_wheel(
                &build_context,
                &python,
                venv_dir,
                pip_path.as_deref(),
                filename,
            )?;
            eprintln!(
                "🛠 Installed {}-{}",
                build_context.metadata21.name, build_context.metadata21.version
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use super::parse_direct_url_path;

    #[test]
    fn test_parse_direct_url() {
        let example_with_direct_url = "\
Name: my-project
Version: 0.1.0
Location: /foo bar/venv/lib/pythonABC/site-packages
Editable project location: /tmp/temporary.whl
Files:
  my_project-0.1.0+abc123de.dist-info/INSTALLER
  my_project-0.1.0+abc123de.dist-info/METADATA
  my_project-0.1.0+abc123de.dist-info/RECORD
  my_project-0.1.0+abc123de.dist-info/REQUESTED
  my_project-0.1.0+abc123de.dist-info/WHEEL
  my_project-0.1.0+abc123de.dist-info/direct_url.json
  my_project-0.1.0+abc123de.dist-info/entry_points.txt
  my_project.pth
";
        assert_eq!(parse_direct_url_path(example_with_direct_url).unwrap(), Some(PathBuf::from("/foo bar/venv/lib/pythonABC/site-packages/my_project-0.1.0+abc123de.dist-info/direct_url.json")));

        let example_without_direct_url = "\
Name: my-project
Version: 0.1.0
Location: /foo bar/venv/lib/pythonABC/site-packages
Files:
  my_project-0.1.0+abc123de.dist-info/INSTALLER
  my_project-0.1.0+abc123de.dist-info/METADATA
  my_project-0.1.0+abc123de.dist-info/RECORD
  my_project-0.1.0+abc123de.dist-info/REQUESTED
  my_project-0.1.0+abc123de.dist-info/WHEEL
  my_project-0.1.0+abc123de.dist-info/entry_points.txt
  my_project.pth
";

        assert_eq!(
            parse_direct_url_path(example_without_direct_url).unwrap(),
            None
        );
    }
}
