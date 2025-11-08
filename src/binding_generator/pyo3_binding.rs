use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use fs_err as fs;
use tracing::debug;
use tracing::instrument;

use crate::ModuleWriter;
use crate::PyProjectToml;
use crate::PythonInterpreter;
use crate::Target;
use crate::module_writer::ModuleWriterExt;
use crate::module_writer::write_python_part;
use crate::project_layout::ProjectLayout;

// Extract the shared object from a AIX big library archive
fn unpack_big_archive(target: &Target, artifact: &Path, temp_dir_path: &Path) -> Result<PathBuf> {
    // Newer rust generates archived dylibs on AIX, as shared
    // libraries are typically archived on the platform.
    if target.cross_compiling() {
        bail!("can't unpack big archive format while cross_compiling")
    }
    debug!("Unpacking archive {}", artifact.display());
    let mut ar_command = Command::new("ar");
    ar_command
        .current_dir(temp_dir_path)
        .arg("-X64")
        .arg("x")
        .arg(artifact);
    let status = ar_command.status().expect("Failed to run ar");
    if !status.success() {
        bail!(r#"ar finished with "{}": `{:?}`"#, status, ar_command,)
    }
    let unpacked_artifact = temp_dir_path.join(artifact.with_extension("so").file_name().unwrap());
    Ok(unpacked_artifact)
}

/// Copies the shared library into the module, which is the only extra file needed with bindings
#[allow(clippy::too_many_arguments)]
#[instrument(skip_all)]
pub fn write_bindings_module(
    writer: &mut impl ModuleWriter,
    project_layout: &ProjectLayout,
    artifact: &Path,
    python_interpreter: Option<&PythonInterpreter>,
    is_abi3: bool,
    target: &Target,
    editable: bool,
    pyproject_toml: Option<&PyProjectToml>,
) -> Result<()> {
    let ext_name = &project_layout.extension_name;
    let so_filename = if is_abi3 {
        if target.is_unix() {
            if target.is_cygwin() {
                format!("{ext_name}.abi3.dll")
            } else {
                format!("{ext_name}.abi3.so")
            }
        } else {
            match python_interpreter {
                Some(python_interpreter) if python_interpreter.is_windows_debug() => {
                    format!("{ext_name}_d.pyd")
                }
                // Apparently there is no tag for abi3 on windows
                _ => format!("{ext_name}.pyd"),
            }
        }
    } else {
        let python_interpreter =
            python_interpreter.expect("A python interpreter is required for non-abi3 build");
        python_interpreter.get_library_name(ext_name)
    };

    let artifact_is_big_ar =
        target.is_aix() && artifact.extension().unwrap_or(OsStr::new(" ")) == OsStr::new("a");
    let temp_dir = if artifact_is_big_ar {
        Some(tempfile::tempdir()?)
    } else {
        None
    };
    let artifact_buff = if artifact_is_big_ar {
        Some(unpack_big_archive(
            target,
            artifact,
            temp_dir.as_ref().unwrap().path(),
        )?)
    } else {
        None
    };
    let artifact = if artifact_is_big_ar {
        artifact_buff.as_ref().unwrap()
    } else {
        artifact
    };

    if !editable {
        write_python_part(writer, project_layout, pyproject_toml)
            .context("Failed to add the python module to the package")?;
    }
    if let Some(python_module) = &project_layout.python_module {
        if editable {
            let target = project_layout.rust_module.join(&so_filename);
            // Remove existing so file to avoid triggering SIGSEV in running process
            // See https://github.com/PyO3/maturin/issues/758
            debug!("Removing {}", target.display());
            let _ = fs::remove_file(&target);

            debug!("Copying {} to {}", artifact.display(), target.display());
            fs::copy(artifact, &target).context(format!(
                "Failed to copy {} to {}",
                artifact.display(),
                target.display()
            ))?;
        } else {
            let relative = project_layout
                .rust_module
                .strip_prefix(python_module.parent().unwrap())
                .unwrap();
            writer.add_file(relative.join(&so_filename), artifact, true)?;
        }
    } else {
        let module = PathBuf::from(ext_name);
        // Reexport the shared library as if it were the top level module
        writer.add_data(
            module.join("__init__.py"),
            None,
            format!(
                r#"from .{ext_name} import *

__doc__ = {ext_name}.__doc__
if hasattr({ext_name}, "__all__"):
    __all__ = {ext_name}.__all__"#
            )
            .as_bytes(),
            false,
        )?;
        let type_stub = project_layout.rust_module.join(format!("{ext_name}.pyi"));
        if type_stub.exists() {
            eprintln!("📖 Found type stub file at {ext_name}.pyi");
            writer.add_file(module.join("__init__.pyi"), type_stub, false)?;
            writer.add_empty_file(module.join("py.typed"))?;
        }
        writer.add_file(module.join(so_filename), artifact, true)?;
    }

    Ok(())
}
