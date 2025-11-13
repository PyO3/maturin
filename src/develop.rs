use crate::BuildContext;
use crate::BuildOptions;
use crate::PlatformTag;
use crate::PythonInterpreter;
use crate::Target;
use crate::auditwheel::AuditWheelMode;
use crate::build_options::CargoOptions;
use crate::compression::CompressionOptions;
use crate::target::detect_arch_from_python;
use anyhow::ensure;
use anyhow::{Context, Result, anyhow, bail};
use cargo_options::heading;
use fs_err as fs;
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

    fn version(&self, python_path: &Path) -> Result<semver::Version> {
        let mut cmd = self.make_command(python_path);

        // Newer versions of uv no longer support `uv pip --version`, and instead
        // require that we use `uv --version`. This is a workaround to get the
        // version of the install backend for both old and new versions of uv.
        cmd = match self {
            InstallBackend::Pip { .. } => cmd,
            InstallBackend::Uv { path, args } => {
                let mut cmd = Command::new(path);
                cmd.args(args);
                cmd
            }
        };
        let output = cmd
            .arg("--version")
            .output()
            .context("failed to get version of install backend")?;
        ensure!(
            output.status.success(),
            "failed to get version of install backend"
        );
        let stdout = str::from_utf8(&output.stdout)?;
        let re = match self {
            InstallBackend::Pip { .. } => Regex::new(r"pip ([\w\.]+).*"),
            InstallBackend::Uv { .. } => Regex::new(r"uv ([\w\.]+).*"),
        };
        match re.expect("regex should be valid").captures(stdout) {
            Some(captures) => Ok(semver::Version::parse(&captures[1])
                .with_context(|| format!("failed to parse semver from {stdout:?}"))?),
            _ => {
                bail!("failed to parse version from {:?}", stdout);
            }
        }
    }

    /// check whether this install backend supports `show --files`. Returns Ok(()) if it does.
    fn check_supports_show_files(&self, python_path: &Path) -> Result<()> {
        match self {
            InstallBackend::Pip { .. } => Ok(()),
            InstallBackend::Uv { .. } => {
                // https://github.com/astral-sh/uv/releases/tag/0.4.25
                let version = self.version(python_path)?;
                if version < semver::Version::new(0, 4, 25) {
                    bail!(
                        "uv >= 0.4.25 is required for `show --files`. Version {} was found.",
                        version
                    );
                }
                Ok(())
            }
        }
    }

    fn stderr_indicates_problem(&self) -> bool {
        match self {
            InstallBackend::Pip { .. } => true,
            // `uv pip install` sends regular logs to stderr, not just errors
            InstallBackend::Uv { .. } => false,
        }
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

/// Check if a virtualenv is created by uv by reading pyvenv.cfg
fn is_uv_venv(venv_dir: &Path) -> bool {
    let pyvenv_cfg = venv_dir.join("pyvenv.cfg");
    if !pyvenv_cfg.exists() {
        return false;
    }
    match fs::read_to_string(&pyvenv_cfg) {
        Ok(content) => content.contains("\nuv = "),
        Err(_) => false,
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
    #[arg(short = 'r', long, help_heading = heading::COMPILATION_OPTIONS, conflicts_with = "profile")]
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

    /// Wheel compression options
    #[command(flatten)]
    pub compression: CompressionOptions,
}

#[instrument(skip_all)]
fn install_dependencies(
    build_context: &BuildContext,
    extras: &[String],
    python: &Path,
    venv_dir: &Path,
    install_backend: &InstallBackend,
) -> Result<()> {
    if !build_context.metadata24.requires_dist.is_empty() {
        let mut extra_names = Vec::with_capacity(extras.len());
        for extra in extras {
            extra_names.push(
                pep508_rs::ExtraName::new(extra.clone())
                    .with_context(|| format!("invalid extra name: {extra}"))?,
            );
        }
        let mut args = vec!["install".to_string()];
        args.extend(build_context.metadata24.requires_dist.iter().map(|x| {
            let mut pkg = x.clone();
            // Remove extra marker to make it installable with pip:
            //
            // * ` and extra == 'EXTRA_NAME'`
            // * `; extra == 'EXTRA_NAME'`
            //
            // Keep in sync with `Metadata23::merge_pyproject_toml()`
            pkg.marker = pkg.marker.simplify_extras(&extra_names);
            pkg.to_string()
        }));
        let status = install_backend
            .make_command(python)
            .args(&args)
            .env("VIRTUAL_ENV", venv_dir)
            .status()
            .with_context(|| format!("Failed to run {} install", install_backend.name()))?;
        if !status.success() {
            bail!(
                r#"{} install finished with "{}""#,
                install_backend.name(),
                status
            )
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
        .env("VIRTUAL_ENV", venv_dir)
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
    if !output.stderr.is_empty() && install_backend.stderr_indicates_problem() {
        eprintln!(
            "⚠️ Warning: {} raised a warning running {:?}:\n{}",
            install_backend.name(),
            &cmd.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    if let Err(err) = configure_as_editable(build_context, python, install_backend) {
        eprintln!("⚠️ Warning: failed to set package as editable: {err}");
    }
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
fn configure_as_editable(
    build_context: &BuildContext,
    python: &Path,
    install_backend: &InstallBackend,
) -> Result<()> {
    println!("✏️ Setting installed package as editable");
    install_backend.check_supports_show_files(python)?;
    let mut cmd = install_backend.make_command(python);
    let cmd = cmd.args(["show", "--files", &build_context.metadata24.name]);
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
        mut cargo_options,
        uv,
        compression,
    } = develop_options;
    compression.validate();

    // set profile to release if specified; `--release` and `--profile` are mutually exclusive
    if release {
        cargo_options.profile = Some("release".to_string());
    }

    let mut target_triple = cargo_options.target.clone();
    let target = Target::from_target_triple(cargo_options.target.as_ref())?;
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
        compression,
    };

    let build_context = build_options
        .into_build_context()
        .strip(strip)
        .editable(true)
        .build()?;

    // Ensure that version information is present, https://github.com/PyO3/maturin/issues/2416
    if build_context
        .pyproject_toml
        .as_ref()
        .is_some_and(|p| !p.warn_invalid_version_info())
    {
        bail!(
            "Cannot build without valid version information. \
               You need to specify either `project.version` or `project.dynamic = [\"version\"]` in pyproject.toml."
        );
    }

    let interpreter =
        PythonInterpreter::check_executable(&python, &target, build_context.bridge())?.ok_or_else(
            || anyhow!("Expected `python` to be a python interpreter inside a virtualenv ಠ_ಠ"),
        )?;

    let uv_venv = is_uv_venv(venv_dir);
    let uv_info = if uv || uv_venv {
        match find_uv_python(&interpreter.executable).or_else(|_| find_uv_bin()) {
            Ok(uv_info) => Some(Ok(uv_info)),
            Err(e) => {
                if uv {
                    Some(Err(e))
                } else {
                    // Ignore error and try pip instead if it's a uv venv but `--uv` is not specified
                    None
                }
            }
        }
    } else {
        None
    };
    let install_backend = if let Some(uv_info) = uv_info {
        let (uv_path, uv_args) = uv_info?;
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

    if !skip_install {
        install_dependencies(&build_context, &extras, &python, venv_dir, &install_backend)?;
    }

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
                build_context.metadata24.name, build_context.metadata24.version
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
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
        let expected_path = PathBuf::from(
            "/foo bar/venv/lib/pythonABC/site-packages/my_project-0.1.0+abc123de.dist-info/direct_url.json",
        );
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

        let expected_path = PathBuf::from(
            "C:\\foo bar\\venv\\Lib\\site-packages\\my_project-0.1.0+abc123de.dist-info\\direct_url.json",
        );
        assert_eq!(
            parse_direct_url_path(example_with_direct_url_windows).unwrap(),
            Some(expected_path)
        );
    }
}
