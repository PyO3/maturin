use crate::auditwheel::AuditWheelMode;
use crate::build_options::CargoOptions;
use crate::target::detect_arch_from_python;
use crate::BuildContext;
use crate::BuildOptions;
use crate::PlatformTag;
use crate::PythonInterpreter;
use crate::Target;
use anyhow::ensure;
use anyhow::{anyhow, bail, Context, Result};
use cargo_options::heading;
use fs_err as fs;
use pep508_rs::{MarkerExpression, MarkerOperator, MarkerTree, MarkerValue};
use regex::Regex;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str;
use tempfile::TempDir;
use tracing::{debug, instrument};
use url::Url;

enum InstallBackend {
    Pip {
        path: Option<PathBuf>,
    },
    Uv {
        path: PathBuf,
        args: Vec<&'static str>,
    },
}

impl InstallBackend {
    fn name(&self) -> &'static str {
        match self {
            InstallBackend::Pip { .. } => "pip",
            InstallBackend::Uv { .. } => "uv pip",
        }
    }

    fn is_pip(&self) -> bool {
        matches!(self, InstallBackend::Pip { .. })
    }

    fn make_command(&self, python_path: &Path) -> Command {
        match self {
            InstallBackend::Pip { path } => match &path {
                Some(path) => {
                    let mut cmd = Command::new(path);
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
            },
            InstallBackend::Uv { path, args } => {
                let mut cmd = Command::new(path);
                cmd.args(args).arg("pip");
                cmd
            }
        }
    }
}

/// Detect the plain uv binary
fn find_uv_bin() -> Result<(PathBuf, Vec<&'static str>)> {
    let output = Command::new("uv").arg("--version").output()?;
    if output.status.success() {
        let version_str =
            str::from_utf8(&output.stdout).context("`uv --version` didn't return utf8 output")?;
        debug!(version = %version_str, "Found uv binary in PATH");
        Ok((PathBuf::from("uv"), Vec::new()))
    } else {
        bail!("`uv --version` failed with status: {}", output.status);
    }
}

fn check_pip_exists(python_path: &Path, pip_path: Option<&PathBuf>) -> Result<()> {
    let output = if let Some(pip_path) = pip_path {
        Command::new(pip_path).args(["--version"]).output()?
    } else {
        Command::new(python_path)
            .args(["-m", "pip", "--version"])
            .output()?
    };
    if output.status.success() {
        let version_str =
            str::from_utf8(&output.stdout).context("`pip --version` didn't return utf8 output")?;
        debug!(version = %version_str, "Found pip");
        Ok(())
    } else {
        bail!("`pip --version` failed with status: {}", output.status);
    }
}

/// Detect the Python uv package
fn find_uv_python(python_path: &Path) -> Result<(PathBuf, Vec<&'static str>)> {
    let output = Command::new(python_path)
        .args(["-m", "uv", "--version"])
        .output()?;
    if output.status.success() {
        let version_str =
            str::from_utf8(&output.stdout).context("`uv --version` didn't return utf8 output")?;
        debug!(version = %version_str, "Found Python uv module");
        Ok((python_path.to_path_buf(), vec!["-m", "uv"]))
    } else {
        bail!(
            "`{} -m uv --version` failed with status: {}",
            python_path.display(),
            output.status
        );
    }
}

/// Install the crate as module in the current virtualenv
#[derive(Debug, clap::Parser)]
pub struct DevelopOptions {
    /// Which kind of bindings to use
    #[arg(
        short = 'b',
        long = "bindings",
        alias = "binding-crate",
        value_parser = ["pyo3", "pyo3-ffi", "cffi", "uniffi", "bin"]
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
    /// Use `uv` to install packages instead of `pip`
    #[arg(long)]
    pub uv: bool,
}

#[instrument(skip_all)]
fn install_dependencies(
    build_context: &BuildContext,
    extras: &[String],
    interpreter: &PythonInterpreter,
    install_backend: &InstallBackend,
) -> Result<()> {
    if !build_context.metadata23.requires_dist.is_empty() {
        let mut args = vec!["install".to_string()];
        args.extend(build_context.metadata23.requires_dist.iter().map(|x| {
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
        let status = install_backend
            .make_command(&interpreter.executable)
            .args(&args)
            .status()
            .context("Failed to run pip install")?;
        if !status.success() {
            bail!(r#"pip install finished with "{}""#, status)
        }
    }
    Ok(())
}

#[instrument(skip_all, fields(wheel_filename = %wheel_filename.display()))]
fn install_wheel(
    build_context: &BuildContext,
    python: &Path,
    venv_dir: &Path,
    wheel_filename: &Path,
    install_backend: &InstallBackend,
) -> Result<()> {
    let mut cmd = install_backend.make_command(python);
    let output = cmd
        .args(["install", "--no-deps", "--force-reinstall"])
        .arg(dunce::simplified(wheel_filename))
        .output()
        .context(format!(
            "{} install failed (ran {:?} with {:?})",
            install_backend.name(),
            cmd.get_program(),
            &cmd.get_args().collect::<Vec<_>>(),
        ))?;
    if !output.status.success() {
        bail!(
            "{} install in {} failed running {:?}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\n",
            install_backend.name(),
            venv_dir.display(),
            &cmd.get_args().collect::<Vec<_>>(),
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    // uv pip install sends logs to stderr thus only print this warning for pip
    if !output.stderr.is_empty() && install_backend.is_pip() {
        eprintln!(
            "⚠️ Warning: pip raised a warning running {:?}:\n{}",
            &cmd.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    fix_direct_url(build_context, python, install_backend)?;
    Ok(())
}

/// Each editable-installed python package has a `direct_url.json` file that includes a `file://` URL
/// indicating the location of the source code of that project. The maturin import hook uses this
/// URL to locate and rebuild editable-installed projects.
///
/// When a maturin package is installed using `pip install -e`, pip takes care of writing the
/// correct URL, however when a maturin package is installed with `maturin develop`, the URL is
/// set to the path to the temporary wheel file created during installation.
#[instrument(skip_all)]
fn fix_direct_url(
    build_context: &BuildContext,
    python: &Path,
    install_backend: &InstallBackend,
) -> Result<()> {
    println!("✏️  Setting installed package as editable");
    let mut cmd = install_backend.make_command(python);
    let cmd = cmd.args(["show", "--files", &build_context.metadata23.name]);
    debug!("running {:?}", cmd);
    let output = cmd.output()?;
    ensure!(output.status.success(), "failed to list package files");
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
    if let Some(Some(location)) = Regex::new(r"Location: ([^\r\n]*)")?
        .captures(pip_show_output)
        .map(|c| c.get(1))
    {
        if let Some(Some(direct_url_path)) = Regex::new(r"  (.*direct_url.json)")?
            .captures(pip_show_output)
            .map(|c| c.get(1))
        {
            return Ok(Some(
                PathBuf::from(location.as_str()).join(direct_url_path.as_str()),
            ));
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
        uv,
    } = develop_options;
    let mut target_triple = cargo_options.target.as_ref().map(|x| x.to_string());
    let target = Target::from_target_triple(cargo_options.target)?;
    let python = target.get_venv_python(venv_dir);

    // check python platform and architecture
    if !target.user_specified {
        if let Some(detected_target) = detect_arch_from_python(&python, &target) {
            target_triple = Some(detected_target);
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
        auditwheel: Some(AuditWheelMode::Skip),
        skip_auditwheel: false,
        #[cfg(feature = "zig")]
        zig: false,
        cargo: CargoOptions {
            target: target_triple,
            ..cargo_options
        },
    };

    let build_context = build_options
        .into_build_context()
        .release(release)
        .strip(strip)
        .editable(true)
        .build()?;

    let interpreter =
        PythonInterpreter::check_executable(&python, &target, build_context.bridge())?.ok_or_else(
            || anyhow!("Expected `python` to be a python interpreter inside a virtualenv ಠ_ಠ"),
        )?;

    let install_backend = if uv {
        let (uv_path, uv_args) = find_uv_python(&interpreter.executable)
            .or_else(|_| find_uv_bin())
            .context("Failed to find uv")?;
        InstallBackend::Uv {
            path: uv_path,
            args: uv_args,
        }
    } else {
        check_pip_exists(&interpreter.executable, pip_path.as_ref())
            .context("Failed to find pip (if working with a uv venv try `maturin develop --uv`)")?;
        InstallBackend::Pip {
            path: pip_path.clone(),
        }
    };

    install_dependencies(&build_context, &extras, &interpreter, &install_backend)?;

    let wheels = build_context.build_wheels()?;
    if !skip_install {
        for (filename, _supported_version) in wheels.iter() {
            install_wheel(
                &build_context,
                &python,
                venv_dir,
                filename,
                &install_backend,
            )?;
            eprintln!(
                "🛠 Installed {}-{}",
                build_context.metadata23.name, build_context.metadata23.version
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
    #[cfg(not(target_os = "windows"))]
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
        let expected_path = PathBuf::from("/foo bar/venv/lib/pythonABC/site-packages/my_project-0.1.0+abc123de.dist-info/direct_url.json");
        assert_eq!(
            parse_direct_url_path(example_with_direct_url).unwrap(),
            Some(expected_path)
        );

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

    #[test]
    #[cfg(target_os = "windows")]
    fn test_parse_direct_url_windows() {
        let example_with_direct_url_windows = "\
Name: my-project\r
Version: 0.1.0\r
Location: C:\\foo bar\\venv\\Lib\\site-packages\r
Files:\r
  my_project-0.1.0+abc123de.dist-info\\INSTALLER\r
  my_project-0.1.0+abc123de.dist-info\\METADATA\r
  my_project-0.1.0+abc123de.dist-info\\RECORD\r
  my_project-0.1.0+abc123de.dist-info\\REQUESTED\r
  my_project-0.1.0+abc123de.dist-info\\WHEEL\r
  my_project-0.1.0+abc123de.dist-info\\direct_url.json\r
  my_project-0.1.0+abc123de.dist-info\\entry_points.txt\r
  my_project.pth\r
";

        let expected_path = PathBuf::from("C:\\foo bar\\venv\\Lib\\site-packages\\my_project-0.1.0+abc123de.dist-info\\direct_url.json");
        assert_eq!(
            parse_direct_url_path(example_with_direct_url_windows).unwrap(),
            Some(expected_path)
        );
    }
}
