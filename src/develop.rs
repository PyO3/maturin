use crate::BuildOptions;
use crate::PlatformTag;
use crate::PythonInterpreter;
use crate::Target;
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::process::Command;
use std::str;

/// Installs a crate by compiling it and copying the shared library to site-packages.
/// Also adds the dist-info directory to make sure pip and other tools detect the library
///
/// Works only in a virtualenv.
#[allow(clippy::too_many_arguments)]
pub fn develop(
    bindings: Option<String>,
    manifest_file: &Path,
    cargo_extra_args: Vec<String>,
    rustc_extra_args: Vec<String>,
    venv_dir: &Path,
    release: bool,
    strip: bool,
    extras: Vec<String>,
) -> Result<()> {
    let target = Target::from_target_triple(None)?;

    let python = target.get_venv_python(&venv_dir);

    let build_options = BuildOptions {
        platform_tag: Some(PlatformTag::Linux),
        interpreter: Some(vec![python.clone()]),
        bindings,
        manifest_path: manifest_file.to_path_buf(),
        out: None,
        skip_auditwheel: false,
        target: None,
        cargo_extra_args,
        rustc_extra_args,
        universal2: false,
    };

    let build_context = build_options.into_build_context(release, strip, true)?;

    let interpreter = PythonInterpreter::check_executable(&python, &target, &build_context.bridge)?
        .ok_or_else(|| {
            anyhow!("Expected `python` to be a python interpreter inside a virtualenv ಠ_ಠ")
        })?;

    // Install dependencies
    if !build_context.metadata21.requires_dist.is_empty() {
        let mut args = vec!["-m".to_string(), "pip".to_string(), "install".to_string()];
        args.extend(build_context.metadata21.requires_dist.iter().map(|x| {
            let mut pkg = x.clone();
            // Remove extra marker to make it installable with pip
            for extra in &extras {
                pkg = pkg
                    .replace(&format!(" and extra == '{}'", extra), "")
                    .replace(&format!("; extra == '{}'", extra), "");
            }
            pkg
        }));
        let status = Command::new(&interpreter.executable)
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
            "--force-reinstall",
            &adjust_canonicalization(filename),
        ];
        let output = Command::new(&python)
            .args(&command)
            .output()
            .context(format!("pip install failed with {:?}", python))?;
        if !output.status.success() {
            bail!(
                "pip install in {} failed running {:?}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\n",
                venv_dir.display(),
                &command,
                output.status,
                str::from_utf8(&output.stdout)?.trim(),
                str::from_utf8(&output.stderr)?.trim(),
            );
        }
        if !output.stderr.is_empty() {
            eprintln!(
                "⚠️  Warning: pip raised a warning running {:?}:\n{}",
                &command,
                str::from_utf8(&output.stderr)?.trim(),
            );
        }
        println!(
            "🛠  Installed {}-{}",
            build_context.metadata21.name, build_context.metadata21.version
        );
    }

    Ok(())
}

// Y U NO accept windows path prefix, pip?
// Anyways, here's shepmasters stack overflow solution
// https://stackoverflow.com/a/50323079/3549270
#[cfg(not(target_os = "windows"))]
fn adjust_canonicalization(p: impl AsRef<Path>) -> String {
    p.as_ref().display().to_string()
}

#[cfg(target_os = "windows")]
fn adjust_canonicalization(p: impl AsRef<Path>) -> String {
    const VERBATIM_PREFIX: &str = r#"\\?\"#;
    let p = p.as_ref().display().to_string();
    if p.starts_with(VERBATIM_PREFIX) {
        p[VERBATIM_PREFIX.len()..].to_string()
    } else {
        p
    }
}
