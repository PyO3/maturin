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
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use tracing::{debug, warn};

use crate::BuildArtifact;
use crate::BuildContext;
use crate::ModuleWriter;
use crate::VirtualWriter;
use crate::WheelWriter;
use crate::archive_source::ArchiveSource;
#[cfg(unix)]
use crate::module_writer::default_permission;
use crate::module_writer::write_python_part;
use walkdir::WalkDir;

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
pub(crate) enum ArtifactTarget {
    /// A binary executable that should be installed in scripts
    Binary(PathBuf),
    /// An extension module
    ExtensionModule(PathBuf),
}

impl ArtifactTarget {
    pub(crate) fn path(&self) -> &Path {
        match self {
            ArtifactTarget::Binary(path) | ArtifactTarget::ExtensionModule(path) => path,
        }
    }
}

#[derive(Debug)]
pub(crate) struct GeneratorOutput {
    /// The path, relative to the archive root, where the built artifact/module
    /// should be installed
    artifact_target: ArtifactTarget,

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
    generator: &mut (impl BindingGenerator + ?Sized),
    context: &BuildContext,
    artifacts: &[A],
    out_dirs: &HashMap<String, PathBuf>,
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
                let source = artifact_source_override.unwrap_or_else(|| artifact.path.clone());
                // Compute the directory where debug info files should be placed.
                // For extension modules (e.g. CFFI mixed projects) the artifact
                // may live in a subdirectory of base_path, so we derive the
                // debug info directory from the artifact's installed location to
                // keep the .dSYM / .pdb / .dwp next to the library it belongs to.
                let debuginfo_base = match &artifact_target {
                    ArtifactTarget::Binary(_) => base_path.clone(),
                    ArtifactTarget::ExtensionModule(path) => base_path
                        .join(path)
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| base_path.clone()),
                };
                match artifact_target {
                    ArtifactTarget::Binary(path) => {
                        // Use add_file_force to bypass exclusion checks for the compiled artifact
                        writer.add_file_force(path, source, true)?;
                    }
                    ArtifactTarget::ExtensionModule(path) => {
                        let target = base_path.join(path);
                        debug!("Removing previously built module {}", target.display());
                        fs::create_dir_all(target.parent().unwrap())?;
                        // Remove existing so file to avoid triggering SIGSEV in running process
                        // See https://github.com/PyO3/maturin/issues/758
                        let _ = fs::remove_file(&target);

                        // 2a. Install the artifact
                        debug!("Installing {} from {}", target.display(), source.display());
                        fs::copy(&source, &target).with_context(|| {
                            format!(
                                "Failed to copy {} to {}",
                                source.display(),
                                target.display(),
                            )
                        })?;
                    }
                }

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

                // 4a. Install import library on Windows
                if let Some(import_lib) = &artifact.import_lib_path
                    && context.include_import_lib
                {
                    let target = base_path.join(import_lib.file_name().unwrap());
                    fs::create_dir_all(target.parent().unwrap())?;
                    debug!("Installing import library {}", target.display());
                    fs::copy(import_lib, &target)?;
                }

                // 5a. Install debug info files
                if let Some(debuginfo) = &artifact.debuginfo_path
                    && context.include_debuginfo
                {
                    install_debuginfo_editable(debuginfo, &debuginfo_base)?;
                }
            }
            _ => {
                // 2b. Install the artifact
                let source = artifact_source_override.unwrap_or_else(|| artifact.path.clone());
                debug!(
                    "Adding to archive {} from {}",
                    artifact_target.path().display(),
                    source.display()
                );
                // Use add_file_force to bypass exclusion checks for the compiled artifact
                writer.add_file_force(artifact_target.path(), source, true)?;

                // 3b. Install additional files
                if let Some(additional_files) = additional_files {
                    for (target, source) in additional_files {
                        debug!("Generating archive entry {}", target.display());
                        // Use add_entry_force to bypass exclusion checks for generated binding files
                        writer.add_entry_force(target, source)?;
                    }
                }

                // 4b. Install import library on Windows
                if let Some(import_lib) = &artifact.import_lib_path
                    && context.include_import_lib
                {
                    let dest = module.join(import_lib.file_name().unwrap());
                    debug!("Adding import library to archive {}", dest.display());
                    writer.add_file_force(dest, import_lib, false)?;
                }

                // 5b. Install debug info files
                if let Some(debuginfo) = &artifact.debuginfo_path
                    && context.include_debuginfo
                {
                    // Binary artifacts go into the `.data/scripts/` directory.
                    // The wheel spec only permits regular files in `scripts/`, so
                    // a `.dSYM` directory bundle (macOS debug info) cannot be
                    // placed there.  Skip debug-info installation for binaries to
                    // avoid producing an invalid wheel that tools like uv reject.
                    if matches!(artifact_target, ArtifactTarget::Binary(_)) {
                        warn!(
                            "Skipping debug info for binary artifact in wheel: {} \
                             (directory bundles such as .dSYM are not permitted in \
                             the wheel `scripts` directory)",
                            debuginfo.display()
                        );
                    } else {
                        // Place debug info next to the artifact inside the wheel,
                        // not at the top-level module (the artifact may be nested
                        // in a subdirectory for CFFI/UniFFI mixed projects).
                        let debuginfo_dir = artifact_target
                            .path()
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| module.clone());
                        install_debuginfo_wheel(writer, debuginfo, &debuginfo_dir)?;
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

    // 5. Include files from OUT_DIR
    if let Some(pyproject) = context.pyproject_toml.as_ref()
        && let Some(glob_patterns) = pyproject.include()
    {
        for inc in glob_patterns.iter().filter_map(|p| p.as_out_dir_include()) {
            let pkg_name = inc.crate_name.unwrap_or(&context.crate_name);
            let out_dir = out_dirs.get(pkg_name).with_context(|| {
                format!(
                    "No OUT_DIR found for crate \"{pkg_name}\". \
                     Make sure the crate has a build script (build.rs)."
                )
            })?;
            eprintln!(
                "📦 Including files matching \"{}\" from OUT_DIR of \"{pkg_name}\"",
                inc.path
            );
            let matches =
                crate::module_writer::glob::resolve_out_dir_includes(inc.path, out_dir, inc.to)?;
            if matches.is_empty() {
                eprintln!(
                    "⚠️  Warning: No files matched \"{}\" in OUT_DIR ({})",
                    inc.path,
                    out_dir.display()
                );
            }
            match (context.editable, &base_path) {
                (true, Some(base_path)) => {
                    for m in matches {
                        let target = base_path.join(&m.target);
                        if let Some(parent) = target.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        debug!(
                            "Installing OUT_DIR file {} from {}",
                            target.display(),
                            m.source.display()
                        );
                        fs::copy(&m.source, &target)?;
                    }
                }
                _ => {
                    for m in matches {
                        #[cfg(unix)]
                        let mode = m.source.metadata()?.permissions().mode();
                        #[cfg(not(unix))]
                        let mode = 0o644;
                        writer.add_file(
                            m.target,
                            m.source,
                            crate::module_writer::permission_is_executable(mode),
                        )?;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Install debug info files for editable installs by copying to the target directory.
/// Handles both single files (.pdb, .dwp) and directory bundles (.dSYM).
fn install_debuginfo_editable(debuginfo: &Path, base_path: &Path) -> Result<()> {
    let debuginfo_name = debuginfo
        .file_name()
        .context("Failed to get debug info file name")?;
    let target = base_path.join(debuginfo_name);

    // Remove stale debuginfo to avoid mixed contents (mirrors the
    // "remove existing .so" logic for the extension module itself)
    if target.is_dir() {
        let _ = fs::remove_dir_all(&target);
    } else if target.exists() {
        let _ = fs::remove_file(&target);
    }

    if debuginfo.is_dir() {
        // .dSYM is a directory bundle on macOS
        debug!(
            "Copying debug info directory {} to {}",
            debuginfo.display(),
            target.display()
        );
        copy_dir_all(debuginfo, &target)?;
    } else if debuginfo.is_file() {
        debug!(
            "Installing debug info {} to {}",
            debuginfo.display(),
            target.display()
        );
        fs::create_dir_all(target.parent().unwrap())?;
        fs::copy(debuginfo, &target)?;
    } else {
        warn!(
            "Debug info path {} is neither a file nor a directory, skipping",
            debuginfo.display()
        );
    }
    Ok(())
}

/// Install debug info files into a wheel archive.
/// Handles both single files (.pdb, .dwp) and directory bundles (.dSYM).
fn install_debuginfo_wheel(
    writer: &mut VirtualWriter<WheelWriter>,
    debuginfo: &Path,
    module: &Path,
) -> Result<()> {
    let debuginfo_name = debuginfo
        .file_name()
        .context("Failed to get debug info file name")?;

    if debuginfo.is_dir() {
        // .dSYM is a directory bundle on macOS — add all files recursively
        for entry in WalkDir::new(debuginfo).follow_links(true) {
            let entry = entry?;
            if entry.file_type().is_file() {
                let relative = entry
                    .path()
                    .strip_prefix(debuginfo)
                    .context("Failed to compute relative path in debug info bundle")?;
                let dest = module.join(debuginfo_name).join(relative);
                debug!(
                    "Adding debug info {} to archive at {}",
                    entry.path().display(),
                    dest.display()
                );
                writer.add_file_force(dest, entry.path(), false)?;
            }
        }
    } else if debuginfo.is_file() {
        let dest = module.join(debuginfo_name);
        debug!(
            "Adding debug info {} to archive at {}",
            debuginfo.display(),
            dest.display()
        );
        writer.add_file_force(dest, debuginfo, false)?;
    } else {
        warn!(
            "Debug info path {} is neither a file nor a directory, skipping",
            debuginfo.display()
        );
    }
    Ok(())
}

/// Recursively copy a directory and its contents.
pub(crate) fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in WalkDir::new(src).follow_links(true) {
        let entry = entry?;
        let relative = entry
            .path()
            .strip_prefix(src)
            .context("Failed to compute relative path")?;
        let target = dst.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
