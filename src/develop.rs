use build_context::BridgeModel;
use compile;
use failure::{Context, Error, ResultExt};
use module_writer::write_bindings_module;
use module_writer::write_cffi_module;
use module_writer::DevelopModuleWriter;
use std::path::Path;
use BuildOptions;
use PythonInterpreter;
use Target;

/// Installs a crate by compiling it and copying the shared library to the right directory
///
/// Works only in virtualenvs.
pub fn develop(
    binding_crate: &Option<String>,
    manifest_file: &Path,
    cargo_extra_args: Vec<String>,
    rustc_extra_args: Vec<String>,
    venv_dir: &Path,
) -> Result<(), Error> {
    let target = Target::current();

    let python = target.get_venv_python(&venv_dir);

    let interpreter = PythonInterpreter::check_executable(python, &target)?.ok_or_else(|| {
        Context::new("Expected `python` to be a python interpreter inside a virtualenv ಠ_ಠ")
    })?;

    let build_options = BuildOptions {
        interpreter: vec!["python".to_string()],
        bindings: binding_crate.clone(),
        manifest_path: manifest_file.to_path_buf(),
        out: None,
        debug: true,
        skip_auditwheel: false,
        cargo_extra_args,
        rustc_extra_args,
    };

    let build_context = build_options.into_build_context()?;

    let mut builder = DevelopModuleWriter::venv(&target, &venv_dir)?;

    let context = "Failed to build a native library through cargo";

    match build_context.bridge {
        BridgeModel::Cffi => {
            let artifact = compile(&build_context, None, None).context(context)?;

            write_cffi_module(
                &mut builder,
                &build_context.module_name,
                &artifact,
                &build_context.target,
            )?;
        }
        BridgeModel::Bindings {
            ref bindings_crate, ..
        } => {
            let artifact = compile(
                &build_context,
                Some(&interpreter),
                Some(bindings_crate.clone()),
            ).context(context)?;

            write_bindings_module(
                &mut builder,
                &build_context.module_name,
                &artifact,
                &interpreter,
            )?;
        }
    }

    Ok(())
}
