use std::borrow::Borrow;
use std::collections::HashMap;
use std::io;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use fs_err as fs;
use fs_err::File;
#[cfg(unix)]
use fs_err::os::unix::fs::OpenOptionsExt as _;
use tracing::debug;

use crate::BuildArtifact;
use crate::BuildContext;
use crate::ModuleWriter;
use crate::VirtualWriter;
use crate::WheelWriter;
use crate::archive_source::ArchiveSource;
#[cfg(unix)]
use crate::module_writer::default_permission;
use crate::module_writer::write_python_part;

mod bin_binding;
mod cffi_binding;
mod pyo3_binding;
mod uniffi_binding;

pub use bin_binding::BinBindingGenerator;
pub use cffi_binding::CffiBindingGenerator;
pub use pyo3_binding::Pyo3BindingGenerator;
pub use uniffi_binding::UniFfiBindingGenerator;

/// A trait to generate the binding files to be included in the built module
///
/// This trait is used to generate the support files necessary to build a python
/// module for any [crate::BridgeModel]
pub(crate) trait BindingGenerator {
    fn generate_bindings(
        &mut self,
        context: &BuildContext,
        artifact: &BuildArtifact,
        module: &Path,
    ) -> Result<GeneratorOutput>;
}

#[derive(Debug)]
pub(crate) struct GeneratorOutput {
    /// The path, relative to the archive root, where the built artifact/module
    /// should be installed
    artifact_target: PathBuf,

    /// In some cases, the source path of the artifact is altered
    /// (e.g. when the build output is an archive which needs to be unpacked)
    artifact_source_override: Option<PathBuf>,

    /// Additional files to be installed (e.g. __init__.py)
    /// The provided PathBuf should be relative to the archive root
    additional_files: Option<HashMap<PathBuf, ArchiveSource>>,
}

/// Every binding generator ultimately has to install the following:
/// 1. The python files (if any)
/// 2. The artifact
/// 3. Additional files
/// 4. Type stubs (if any/pure rust only)
///
/// Additionally, the above are installed to 2 potential locations:
/// 1. The archive
/// 2. The filesystem
///
/// For editable installs:
/// If the project is pure rust, the wheel is built as normal and installed
/// If the project has python, the artifact is installed into the project and a pth is written to the archive
///
/// So the full matrix comes down to:
/// 1. editable, has python => install to fs, write pth to archive
/// 2. everything else => install to archive/build as normal
///
/// Note: Writing the pth to the archive is handled by [BuildContext], not here
pub fn generate_binding<A>(
    writer: &mut VirtualWriter<WheelWriter>,
    generator: &mut impl BindingGenerator,
    context: &BuildContext,
    artifacts: &[A],
) -> Result<()>
where
    A: Borrow<BuildArtifact>,
{
    // 1. Install the python files
    if !context.editable {
        write_python_part(
            writer,
            &context.project_layout,
            context.pyproject_toml.as_ref(),
        )
        .context("Failed to add the python module to the package")?;
    }

    let base_path = context
        .project_layout
        .python_module
        .as_ref()
        .map(|python_module| python_module.parent().unwrap().to_path_buf());

    let module = match &base_path {
        Some(base_path) => context
            .project_layout
            .rust_module
            .strip_prefix(base_path)
            .unwrap()
            .to_path_buf(),
        None => PathBuf::from(&context.project_layout.extension_name),
    };

    for artifact in artifacts {
        let artifact = artifact.borrow();
        let GeneratorOutput {
            artifact_target,
            artifact_source_override,
            additional_files,
        } = generator.generate_bindings(context, artifact, &module)?;

        match (context.editable, &base_path) {
            (true, Some(base_path)) => {
                let target = base_path.join(&artifact_target);
                debug!("Removing previously built module {}", target.display());
                fs::create_dir_all(target.parent().unwrap())?;
                // Remove existing so file to avoid triggering SIGSEV in running process
                // See https://github.com/PyO3/maturin/issues/758
                let _ = fs::remove_file(&target);
                let source = artifact_source_override.unwrap_or_else(|| artifact.path.clone());

                // 2a. Install the artifact
                debug!("Installing {} from {}", target.display(), source.display());
                fs::copy(&source, &target).with_context(|| {
                    format!(
                        "Failed to copy {} to {}",
                        source.display(),
                        target.display(),
                    )
                })?;

                // 3a. Install additional files
                if let Some(additional_files) = additional_files {
                    for (target, source) in additional_files {
                        let target = base_path.join(target);
                        fs::create_dir_all(target.parent().unwrap())?;
                        debug!("Generating file {}", target.display());
                        let mut options = File::options();
                        options.write(true).create(true).truncate(true);
                        #[cfg(unix)]
                        {
                            options.mode(default_permission(source.executable()));
                        }

                        let mut file = options.open(&target)?;
                        match source {
                            ArchiveSource::Generated(source) => file.write_all(&source.data)?,
                            ArchiveSource::File(source) => {
                                let mut source = File::options().read(true).open(source.path)?;
                                io::copy(&mut source, &mut file)?;
                            }
                        }
                    }
                }
            }
            _ => {
                // 2b. Install the artifact
                let source = artifact_source_override.unwrap_or_else(|| artifact.path.clone());
                debug!(
                    "Adding to archive {} from {}",
                    artifact_target.display(),
                    source.display()
                );
                writer.add_file(artifact_target, source, true)?;

                // 3b. Install additional files
                if let Some(additional_files) = additional_files {
                    for (target, source) in additional_files {
                        debug!("Generating archive entry {}", target.display());
                        writer.add_entry(target, source)?;
                    }
                }
            }
        }
    }

    // 4. Install type stubs
    if context.project_layout.python_module.is_none() {
        let ext_name = &context.project_layout.extension_name;
        let type_stub = context
            .project_layout
            .rust_module
            .join(format!("{ext_name}.pyi"));
        if type_stub.exists() {
            eprintln!("📖 Found type stub file at {ext_name}.pyi");
            writer.add_file(module.join("__init__.pyi"), type_stub, false)?;
            writer.add_empty_file(module.join("py.typed"))?;
        }
    }

    Ok(())
}
