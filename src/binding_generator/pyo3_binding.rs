use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use pyo3_introspection::{introspect_cdylib, module_stub_files};
use tempfile::TempDir;
use tracing::debug;

use crate::BuildArtifact;
use crate::BuildContext;
use crate::PythonInterpreter;
use crate::StableAbiKind;
use crate::Target;
use crate::archive_source::ArchiveSource;
use crate::archive_source::GeneratedSourceData;
use crate::binding_generator::ArtifactTarget;

use super::BindingGenerator;
use super::GeneratorOutput;

/// A generator for producing PyO3 bindings.
///
/// This struct is responsible for generating Python bindings for modules using PyO3.
/// The `binding_type` field determines whether the generated bindings use the stable PyO3 "abi3" interface,
/// which allows compatibility with multiple Python versions and allows targeting a specific python
/// interpreter.
pub struct Pyo3BindingGenerator<'a> {
    binding_type: BindingType<'a>,
    tempdir: Rc<TempDir>,
}

enum BindingType<'a> {
    Abi3(Option<&'a PythonInterpreter>),
    VersionSpecific(&'a PythonInterpreter),
}

impl<'a> Pyo3BindingGenerator<'a> {
    pub fn new(
        stable_abi: Option<StableAbiKind>,
        interpreter: Option<&'a PythonInterpreter>,
        tempdir: Rc<TempDir>,
    ) -> Result<Self> {
        let binding_type = match stable_abi {
            Some(kind) => match kind {
                StableAbiKind::Abi3 => BindingType::Abi3(interpreter),
            },
            None => {
                let interpreter = interpreter.ok_or_else(|| {
                    anyhow!(
                    "A python interpreter is required for non-abi3 builds but one was not provided"
                )
                })?;
                BindingType::VersionSpecific(interpreter)
            }
        };
        Ok(Self {
            binding_type,
            tempdir,
        })
    }
}

fn ext_suffix(
    target: &Target,
    interpreter: Option<&PythonInterpreter>,
    ext_name: &str,
    abi_name: &str,
) -> String {
    if target.is_unix() {
        if target.is_cygwin() {
            format!("{ext_name}.{abi_name}.dll")
        } else {
            format!("{ext_name}.{abi_name}.so")
        }
    } else {
        match interpreter {
            Some(interpreter) if interpreter.is_windows_debug() => {
                format!("{ext_name}_d.pyd")
            }
            // Apparently there is no tag for abi3 on windows
            _ => format!("{ext_name}.pyd"),
        }
    }
}

impl<'a> BindingGenerator for Pyo3BindingGenerator<'a> {
    fn generate_bindings(
        &mut self,
        context: &BuildContext,
        artifact: &BuildArtifact,
        module: &Path,
    ) -> Result<GeneratorOutput> {
        let ext_name = &context.project.project_layout.extension_name;
        let target = &context.project.target;

        let so_filename = match self.binding_type {
            BindingType::Abi3(interpreter) => ext_suffix(target, interpreter, ext_name, "abi3"),
            BindingType::VersionSpecific(interpreter) => interpreter.get_library_name(ext_name),
        };
        let artifact_target = ArtifactTarget::ExtensionModule(module.join(so_filename));

        let artifact_is_big_ar = target.is_aix()
            && artifact.path.extension().unwrap_or(OsStr::new(" ")) == OsStr::new("a");

        let artifact_source_override = if artifact_is_big_ar {
            Some(unpack_big_archive(
                target,
                &artifact.path,
                self.tempdir.path(),
            )?)
        } else {
            None
        };

        let additional_files = match context.project.project_layout.python_module {
            Some(_) => None,
            None => {
                let mut files = HashMap::new();
                let source = GeneratedSourceData {
                    data: format!(
                        r#"from .{ext_name} import *

__doc__ = {ext_name}.__doc__
if hasattr({ext_name}, "__all__"):
    __all__ = {ext_name}.__all__"#
                    )
                    .into(),
                    path: None,
                    executable: false,
                };
                files.insert(module.join("__init__.py"), ArchiveSource::Generated(source));
                if context.artifact.generate_stubs {
                    let module_introspection = introspect_cdylib(&artifact.path, ext_name).context("Failed to introspect the built libraries to generate type stubs, have you enabled the \"experimental-inspect\" PyO3 Cargo feature?")?;
                    eprintln!("📖 Type stub extracted from the built binary");
                    for (path, stub_content) in module_stub_files(&module_introspection) {
                        files.insert(
                            module.join(path),
                            ArchiveSource::Generated(GeneratedSourceData {
                                data: stub_content.into_bytes(),
                                path: None,
                                executable: false,
                            }),
                        );
                    }
                    files.insert(
                        module.join("py.typed"),
                        ArchiveSource::Generated(GeneratedSourceData {
                            data: Vec::new(),
                            path: None,
                            executable: false,
                        }),
                    );
                }
                Some(files)
            }
        };

        Ok(GeneratorOutput {
            artifact_target,
            artifact_source_override,
            additional_files,
        })
    }
}

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
    let status = ar_command.status().context("Failed to run ar")?;
    if !status.success() {
        bail!(r#"ar finished with "{}": `{:?}`"#, status, ar_command,)
    }
    let unpacked_artifact = temp_dir_path.join(artifact.with_extension("so").file_name().unwrap());
    Ok(unpacked_artifact)
}
