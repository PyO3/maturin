use crate::build_options::CargoOptions;
use crate::target::Arch;
use crate::BuildOptions;
use crate::PlatformTag;
use crate::PythonInterpreter;
use crate::Target;
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Installs a crate by compiling it and copying the shared library to site-packages.
/// Also adds the dist-info directory to make sure pip and other tools detect the library
///
/// Works only in a virtualenv.
#[allow(clippy::too_many_arguments)]
pub fn develop(
    bindings: Option<String>,
    cargo_options: CargoOptions,
    venv_dir: &Path,
    release: bool,
    strip: bool,
    extras: Vec<String>,
) -> Result<()> {
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
        universal2: false,
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
        let mut args = vec![
            "-m".to_string(),
            "pip".to_string(),
            "install".to_string(),
            "--disable-pip-version-check".to_string(),
        ];
        args.extend(build_context.metadata21.requires_dist.iter().map(|x| {
            let mut pkg = x.clone();
            // Remove extra marker to make it installable with pip
            for extra in &extras {
                pkg = pkg
                    .replace(&format!(" and extra == '{extra}'"), "")
                    .replace(&format!("; extra == '{extra}'"), "");
            }
            pkg
        }));
        let status = Command::new(interpreter.executable)
            .args(&args)
            .status()
            .context("Failed to run pip install")?;
        if !status.success() {
            bail!(r#"pip install finished with "{}""#, status)
        }
    }

    let wheels = build_context.build_wheels()?;
    for (filename, _supported_version) in wheels.iter() {
        let command = [
            "-m",
            "pip",
            "--disable-pip-version-check",
            "install",
            "--no-deps",
            "--force-reinstall",
        ];
        let output = Command::new(&python)
            .args(command)
            .arg(dunce::simplified(filename))
            .output()
            .context(format!("pip install failed with {python:?}"))?;
        if !output.status.success() {
            bail!(
                "pip install in {} failed running {:?}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\n",
                venv_dir.display(),
                &command,
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim(),
            );
        }
        if !output.stderr.is_empty() {
            eprintln!(
                "⚠️ Warning: pip raised a warning running {:?}:\n{}",
                &command,
                String::from_utf8_lossy(&output.stderr).trim(),
            );
        }
        println!(
            "🛠 Installed {}-{}",
            build_context.metadata21.name, build_context.metadata21.version
        );
    }

    Ok(())
}
