use compile;
use failure::{Context, Error, ResultExt};
use std::env;
use std::fs;
use std::path::PathBuf;
use target_info::Target;
use BuildContext;
use PythonInterpreter;

/// Installs a crate by compiling it and copying the shared library to the right directory
///
/// Works only in virtualenvs.
pub fn develop(
    binding_crate: String,
    manifest_file: PathBuf,
    cargo_extra_args: Vec<String>,
    rustc_extra_args: Vec<String>,
) -> Result<(), Error> {
    let venv_dir = match env::var_os("VIRTUAL_ENV") {
        Some(dir) => PathBuf::from(dir),
        None => bail!("You need be inside a virtualenv to use develop (VIRTUALENV isn't set)"),
    };

    let interpreter = PythonInterpreter::check_executable("python")?.ok_or_else(|| {
        Context::new("Expected `python` to be a python interpreter inside a virtualenv ಠ_ಠ")
    })?;

    let build_context = BuildContext {
        interpreter: vec!["python".to_string()],
        binding_crate,
        manifest_path: manifest_file.clone(),
        wheel_dir: None,
        use_cached: false,
        debug: true,
        skip_auditwheel: false,
        cargo_extra_args,
        rustc_extra_args,
    };

    let wheel_metadata = build_context.get_wheel_metadata()?;

    let build_location = compile(
        &wheel_metadata.module_name,
        &manifest_file,
        &build_context,
        &interpreter,
    ).context("Failed to build a native library through cargo")?;

    let python_dir = format!("python{}.{}", interpreter.major, interpreter.minor);
    let filename = format!(
        "{}{}",
        wheel_metadata.module_name,
        &interpreter.get_library_extension()
    );

    let target_location = match Target::os() {
        "linux" | "macos" => venv_dir
            .join("lib")
            .join(python_dir)
            .join("site-packages")
            .join(filename),
        "windows" => venv_dir.join("Lib").join("site-packages").join(filename),
        unsupported => panic!("Platform {} is not supported", unsupported),
    };

    fs::copy(build_location, target_location)
        .context("Failed to install the libary inside the virtualenv")?;

    Ok(())
}
