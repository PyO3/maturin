use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use fs_err as fs;
use ignore::WalkBuilder;
use indexmap::IndexMap;
use itertools::Itertools as _;
use normpath::PathExt as _;
use tracing::{debug, trace};

use crate::Metadata24;
use crate::PyProjectToml;
use crate::archive_source::ArchiveSource;
use crate::archive_source::FileSourceData;
use crate::archive_source::GeneratedSourceData;
use crate::project_layout::ProjectLayout;
use crate::pyproject_toml::Format;

pub(crate) mod glob;
#[cfg(test)]
mod mock_writer;
mod path_writer;
mod sdist_writer;
mod util;
mod virtual_writer;
mod wheel_writer;

pub use path_writer::PathWriter;
pub use sdist_writer::SDistWriter;
pub use virtual_writer::VirtualWriter;
pub use wheel_writer::WheelWriter;

mod private {
    pub trait Sealed {}
}

/// Allows writing the module to a wheel or add it directly to the virtualenv
pub trait ModuleWriterInternal: private::Sealed {
    /// Adds an entry into the archive
    fn add_entry(&mut self, target: impl AsRef<Path>, source: ArchiveSource) -> Result<()>;
}

/// Extension trait with convenience methods for interacting with a [ModuleWriterInternal]
pub trait ModuleWriter: private::Sealed {
    /// Adds a file with data as content in target relative to the module base path while setting
    /// the appropriate unix permissions
    ///
    /// For generated files, `source` is `None`.
    fn add_bytes(
        &mut self,
        target: impl AsRef<Path>,
        source: Option<&Path>,
        data: impl Into<Vec<u8>>,
        executable: bool,
    ) -> Result<()>;

    /// Copies the source file the target path relative to the module base path while setting
    /// the given unix permissions
    fn add_file(
        &mut self,
        target: impl AsRef<Path>,
        source: impl AsRef<Path>,
        executable: bool,
    ) -> Result<()>;

    /// Add an empty file to the target path
    #[inline]
    fn add_empty_file(&mut self, target: impl AsRef<Path>) -> Result<()> {
        self.add_bytes(target, None, Vec::new(), false)
    }
}

/// This blanket impl makes it impossible to overwrite the methods in [ModuleWriter]
impl<T: ModuleWriterInternal> ModuleWriter for T {
    fn add_bytes(
        &mut self,
        target: impl AsRef<Path>,
        source: Option<&Path>,
        data: impl Into<Vec<u8>>,
        executable: bool,
    ) -> Result<()> {
        self.add_entry(
            target,
            ArchiveSource::Generated(GeneratedSourceData {
                data: data.into(),
                path: source.map(ToOwned::to_owned),
                executable,
            }),
        )
    }

    fn add_file(
        &mut self,
        target: impl AsRef<Path>,
        source: impl AsRef<Path>,
        executable: bool,
    ) -> Result<()> {
        let target = target.as_ref();
        let source = source.as_ref();
        debug!("Adding {} from {}", target.display(), source.display());

        self.add_entry(
            target,
            ArchiveSource::File(FileSourceData {
                path: source.to_path_buf(),
                executable,
            }),
        )
    }
}

/// Adds the python part of a mixed project to the writer,
pub fn write_python_part(
    writer: &mut VirtualWriter<WheelWriter>,
    project_layout: &ProjectLayout,
    pyproject_toml: Option<&PyProjectToml>,
) -> Result<()> {
    let python_dir = &project_layout.python_dir;
    let mut python_packages = Vec::new();
    if let Some(python_module) = project_layout.python_module.as_ref() {
        python_packages.push(python_module.to_path_buf());
    }
    for package in &project_layout.python_packages {
        let package_path = python_dir.join(package);
        if python_packages.contains(&package_path) {
            continue;
        }
        python_packages.push(package_path);
    }

    for absolute in WalkBuilder::new(&project_layout.project_root)
        .hidden(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .build()
    {
        let absolute = match absolute {
            Ok(entry) => entry.into_path(),
            Err(err) => {
                // Skip errors for paths that don't need to be included, e.g. for directories
                // that we don't have permissions for.
                if let ignore::Error::WithPath { path, .. } = &err
                    && !python_packages.iter().any(|pkg| path.starts_with(pkg))
                {
                    // Log priority logging, we're only looking at the directory at all due to
                    // a particularity in how we're doing path traversal.
                    trace!(
                        "Skipping inaccessible path {} due to read error: {err}",
                        path.display()
                    );
                    continue;
                }
                return Err(err.into());
            }
        };
        if !python_packages
            .iter()
            .any(|path| absolute.starts_with(path))
        {
            continue;
        }
        let relative = absolute.strip_prefix(python_dir).unwrap();
        if !absolute.is_dir() {
            if is_develop_build_artifact(relative, &project_layout.extension_name) {
                debug!("Ignoring develop build artifact {}", relative.display());
                continue;
            }
            let mode = file_permission_mode(&absolute)?;
            writer
                .add_file(relative, &absolute, permission_is_executable(mode))
                .context(format!("Failed to add file from {}", absolute.display()))?;
        }
    }

    // Include additional files
    if let Some(pyproject) = pyproject_toml {
        let project_root = &project_layout.project_root;
        if let Some(glob_patterns) = pyproject.include() {
            for pattern in glob_patterns
                .iter()
                .filter_map(|glob_pattern| glob_pattern.targets(Format::Wheel))
            {
                eprintln!("📦 Including files matching \"{pattern}\"");
                let matches = glob::resolve_include_matches(
                    pattern,
                    Format::Wheel,
                    project_root,
                    python_dir,
                )?;
                for m in matches {
                    let mode = file_permission_mode(m.source.as_ref())?;
                    writer.add_file(m.target, m.source, permission_is_executable(mode))?;
                }
            }
        }
    }

    Ok(())
}

/// If any, copies the data files from the data directory, resolving symlinks to their source.
/// We resolve symlinks since we require this rather rigid structure while people might need
/// to save or generate the data in other places
///
/// See https://peps.python.org/pep-0427/#file-contents
pub fn add_data(
    writer: &mut VirtualWriter<WheelWriter>,
    metadata24: &Metadata24,
    data: Option<&Path>,
) -> Result<()> {
    let possible_data_dir_names = ["data", "scripts", "headers", "purelib", "platlib"];
    if let Some(data) = data {
        for subdir in fs::read_dir(data).context("Failed to read data dir")? {
            let subdir = subdir?;
            let dir_name = subdir
                .file_name()
                .to_str()
                .context("Invalid data dir name")?
                .to_string();
            if !subdir.path().is_dir() || !possible_data_dir_names.contains(&dir_name.as_str()) {
                bail!(
                    "Invalid data dir entry {}. Possible are directories named {}",
                    subdir.path().display(),
                    possible_data_dir_names.join(", ")
                );
            }
            debug!("Adding data from {}", subdir.path().display());
            add_data_subdir(writer, subdir.path().as_path(), data, metadata24)
                .with_context(|| format!("Failed to include data from {}", data.display()))?
        }
    }
    Ok(())
}

/// Walk a single data subdirectory and add its files to the writer.
fn add_data_subdir(
    writer: &mut impl ModuleWriter,
    subdir_path: &Path,
    data: &Path,
    metadata24: &Metadata24,
) -> Result<()> {
    for file in WalkBuilder::new(subdir_path)
        .standard_filters(false)
        .build()
    {
        let file = file?;
        let relative_path = file.path().strip_prefix(data).with_context(|| {
            format!(
                "Data file {} is not under data dir {}",
                file.path().display(),
                data.display()
            )
        })?;
        let relative = metadata24.get_data_dir().join(relative_path);

        if file.path_is_symlink() {
            // Copy the actual file contents, not the link, so that you can create a
            // data directory by joining different data sources
            let link_target = fs::read_link(file.path())?;
            let source = if link_target.is_absolute() {
                link_target
            } else {
                file.path()
                    .parent()
                    .with_context(|| {
                        format!(
                            "Data symlink {} has no parent directory",
                            file.path().display()
                        )
                    })?
                    .join(link_target)
            };
            let mode = file_permission_mode(&source)?;
            writer.add_file(relative, source, permission_is_executable(mode))?;
        } else if file.path().is_file() {
            let mode = file_permission_mode(file.path())?;
            writer.add_file(relative, file.path(), permission_is_executable(mode))?;
        } else if file.path().is_dir() {
            // Intentionally ignored
        } else {
            bail!("Can't handle data dir entry {}", file.path().display());
        }
    }
    Ok(())
}

/// Creates the .dist-info directory and fills it with all metadata files except RECORD.
///
/// If the `MATURIN_PEP517_METADATA_DIR` environment variable is set, copies the
/// pre-generated metadata files from that directory instead of regenerating them.
/// The WHEEL file is always regenerated to ensure correct tags. This implements
/// the PEP 517 requirement that `build_wheel` respects `metadata_directory`.
pub fn write_dist_info(
    writer: &mut VirtualWriter<impl ModuleWriterInternal>,
    pyproject_dir: &Path,
    metadata24: &Metadata24,
    tags: &[String],
) -> Result<PathBuf> {
    let dist_info_dir = metadata24.get_dist_info_dir();

    if let Ok(metadata_dir) = std::env::var("MATURIN_PEP517_METADATA_DIR") {
        let metadata_path = Path::new(&metadata_dir);
        // Support both forms:
        //   1. Direct .dist-info path (pip's behavior)
        //   2. Parent directory containing the .dist-info subdirectory (per PEP 517 spec)
        let pre_existing = if metadata_path.is_dir()
            && metadata_path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".dist-info"))
        {
            metadata_path.to_path_buf()
        } else {
            let nested = metadata_path.join(&dist_info_dir);
            if nested.is_dir() {
                nested
            } else {
                bail!(
                    "MATURIN_PEP517_METADATA_DIR is set to '{}' but no .dist-info directory \
                     was found (tried both '{}' directly and '{}')",
                    metadata_dir,
                    metadata_path.display(),
                    metadata_path.join(&dist_info_dir).display(),
                );
            }
        };
        debug!(
            "Using pre-generated metadata from {}",
            pre_existing.display()
        );
        return write_dist_info_from_dir(writer, &dist_info_dir, &pre_existing, tags);
    }

    writer.add_bytes(
        dist_info_dir.join("METADATA"),
        None,
        metadata24.to_file_contents()?.as_bytes(),
        false,
    )?;

    writer.add_bytes(
        dist_info_dir.join("WHEEL"),
        None,
        wheel_file(tags)?.as_bytes(),
        false,
    )?;

    let mut entry_points = String::new();
    if !metadata24.scripts.is_empty() {
        entry_points.push_str(&entry_points_txt("console_scripts", &metadata24.scripts));
    }
    if !metadata24.gui_scripts.is_empty() {
        entry_points.push_str(&entry_points_txt("gui_scripts", &metadata24.gui_scripts));
    }
    for (entry_type, scripts) in &metadata24.entry_points {
        entry_points.push_str(&entry_points_txt(entry_type, scripts));
    }
    if !entry_points.is_empty() {
        writer.add_bytes(
            dist_info_dir.join("entry_points.txt"),
            None,
            entry_points.as_bytes(),
            false,
        )?;
    }

    if !metadata24.license_files.is_empty() {
        let license_files_dir = dist_info_dir.join("licenses");
        for path in &metadata24.license_files {
            if path.is_absolute()
                || path.components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir
                            | std::path::Component::Prefix(_)
                            | std::path::Component::RootDir
                    )
                })
            {
                bail!(
                    "Refusing to write license file with unsafe path `{}` into wheel",
                    path.display()
                );
            }

            let source = metadata24
                .license_file_sources
                .get(path)
                .cloned()
                .unwrap_or_else(|| pyproject_dir.join(path));
            writer.add_file(license_files_dir.join(path), source, false)?;
        }
    }

    Ok(dist_info_dir)
}

/// Copies pre-generated metadata files from a `.dist-info` directory on disk into the wheel,
/// but always regenerates the WHEEL file to ensure correct tags.
fn write_dist_info_from_dir(
    writer: &mut VirtualWriter<impl ModuleWriterInternal>,
    dist_info_dir: &Path,
    source_dir: &Path,
    tags: &[String],
) -> Result<PathBuf> {
    // Always regenerate WHEEL to ensure correct tags for the built wheel
    writer.add_bytes(
        dist_info_dir.join("WHEEL"),
        None,
        wheel_file(tags)?.as_bytes(),
        false,
    )?;

    // Copy all other files from the pre-generated .dist-info directory
    for entry in fs::read_dir(source_dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();

        // Skip WHEEL (already regenerated) and RECORD (will be regenerated by WheelWriter)
        if file_name_str == "WHEEL" || file_name_str == "RECORD" {
            continue;
        }

        let entry_path = entry.path();
        if entry_path.is_dir() {
            // Recursively add subdirectories (e.g. licenses/)
            for sub_entry in walkdir::WalkDir::new(&entry_path) {
                let sub_entry = sub_entry?;
                if sub_entry.file_type().is_file() {
                    let rel = sub_entry.path().strip_prefix(source_dir).with_context(|| {
                        format!(
                            "walkdir entry '{}' is not under source dir '{}'",
                            sub_entry.path().display(),
                            source_dir.display()
                        )
                    })?;
                    writer.add_file(dist_info_dir.join(rel), sub_entry.path(), false)?;
                }
            }
        } else {
            writer.add_file(dist_info_dir.join(&file_name), &entry_path, false)?;
        }
    }

    Ok(dist_info_dir.to_owned())
}

/// Add a pth file to wheel root for editable installs
pub fn write_pth(
    writer: &mut VirtualWriter<WheelWriter>,
    project_layout: &ProjectLayout,
    metadata24: &Metadata24,
) -> Result<()> {
    if project_layout.python_module.is_some() || !project_layout.python_packages.is_empty() {
        let absolute_path = project_layout
            .python_dir
            .normalize()
            .with_context(|| {
                format!(
                    "python dir path `{}` does not exist or is invalid",
                    project_layout.python_dir.display()
                )
            })?
            .into_path_buf();
        if let Some(python_path) = absolute_path.to_str() {
            let name = metadata24.get_distribution_escaped();
            let target = format!("{name}.pth");
            debug!("Adding {} from {}", target, python_path);
            writer.add_bytes(target, None, python_path, false)?;
        } else {
            eprintln!(
                "⚠️ source code path contains non-Unicode sequences, editable installs may not work."
            );
        }
    }
    Ok(())
}

/// Check if a file is a build artifact left behind by `maturin develop`.
///
/// `maturin develop` copies compiled extension modules (`.so`, `.pyd`, `.dll`, `.dylib`)
/// and their associated debug info files (`.pdb`, `.dSYM`, `.dwp`) directly into the
/// Python source tree for editable installs. When `maturin build` later walks the same
/// source tree to collect files for the wheel, these artifacts must be skipped to avoid
/// conflicts with the freshly compiled library being added to the wheel.
///
/// The native library artifacts follow different naming conventions depending on the
/// binding type:
/// - **PyO3/pyo3-ffi**: `{ext_name}.cpython-3XX-*.so`, `{ext_name}.abi3.so`, `{ext_name}.pyd`
/// - **CFFI**: `lib{ext_name}.so`, `lib{ext_name}.dylib` (Unix), `{ext_name}.dll` (Windows)
/// - **UniFFI**: `lib{ext_name}.so`, `lib{ext_name}.dylib` (Unix), `{ext_name}.dll` (Windows)
///
/// Debug info files (`.pdb`, `.dwp`, or files inside `.dSYM` bundles) are also excluded
/// when their name matches the extension name, since they are re-added from the fresh
/// build output when appropriate.
fn is_develop_build_artifact(relative_path: &Path, extension_name: &str) -> bool {
    let Some(file_name) = relative_path.file_name() else {
        return false;
    };
    let file_name = file_name.to_string_lossy();

    // Files inside a .dSYM bundle (macOS debug info directory) — match on the
    // bundle directory name rather than the leaf filename (which can be
    // Info.plist, a DWARF data file, etc.)
    let dsym_bundle = relative_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .find(|c| c.ends_with(".dSYM"));
    if let Some(bundle) = dsym_bundle {
        let bundle = bundle.trim_end_matches(".dSYM");
        return bundle.starts_with(extension_name)
            || bundle.starts_with(&format!("lib{extension_name}"));
    }

    let is_native_ext = file_name.ends_with(".so")
        || file_name.ends_with(".pyd")
        || file_name.ends_with(".dll")
        || file_name.ends_with(".dylib");
    let is_debuginfo = file_name.ends_with(".pdb") || file_name.ends_with(".dwp");
    let name_matches = file_name.starts_with(extension_name)
        || file_name.starts_with(&format!("lib{extension_name}"));
    (is_native_ext || is_debuginfo) && name_matches
}

fn expand_compressed_tag(tag: &str) -> impl Iterator<Item = String> + '_ {
    tag.split('-')
        .map(|component| component.split('.'))
        .multi_cartesian_product()
        .map(|components| components.join("-"))
}

fn wheel_file(tags: &[String]) -> Result<String> {
    let mut wheel_file = format!(
        "Wheel-Version: 1.0
Generator: {name} ({version})
Root-Is-Purelib: false
",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
    );

    // N.B.: Tags should be in expanded form in this metadata (See:
    // https://packaging.python.org/en/latest/specifications/binary-distribution-format/#file-contents
    // items 7 and 11); so we do that expansion here if needed.
    //
    // It might make sense to reify a Tag struct in the code base and then, when a compressed tag
    // set needs to be rendered, render a single string at that time. As things stand though, this
    // is the only place in the codebase that needs reified tags (compressed tag sets expanded); so
    // we do the expansion here.
    //
    // See: https://github.com/PyO3/maturin/issues/2761
    for tag in tags {
        for expanded_tag in expand_compressed_tag(tag) {
            writeln!(wheel_file, "Tag: {expanded_tag}")?;
        }
    }

    Ok(wheel_file)
}

/// https://packaging.python.org/specifications/entry-points/
fn entry_points_txt(
    entry_type: &str,
    entrypoints: &IndexMap<String, String, impl std::hash::BuildHasher>,
) -> String {
    entrypoints
        .iter()
        .fold(format!("[{entry_type}]\n"), |text, (k, v)| {
            text + k + "=" + v + "\n"
        })
}

#[inline]
pub(crate) fn permission_is_executable(mode: u32) -> bool {
    (0o100 & mode) == 0o100
}

/// Returns the Unix permission mode of a file, or 0o644 on non-Unix platforms.
#[inline]
pub(crate) fn file_permission_mode(path: &std::path::Path) -> std::io::Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(path.metadata()?.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(0o644)
    }
}

#[inline]
pub(crate) fn default_permission(executable: bool) -> u32 {
    match executable {
        true => 0o755,
        false => 0o644,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use anyhow::Result;
    use fs_err as fs;
    use ignore::overrides::Override;
    use pep440_rs::Version;
    use tempfile::TempDir;
    use zip::ZipArchive;
    use zip::write::SimpleFileOptions;

    use super::VirtualWriter;
    use super::WheelWriter;
    use super::add_data;
    use super::wheel_file;
    use crate::Metadata24;

    #[test]
    fn wheel_file_compressed_tags() -> Result<()> {
        let expected = format!(
            "Wheel-Version: 1.0
Generator: {name} ({version})
Root-Is-Purelib: false
Tag: py2-none-any
Tag: py3-none-any
Tag: pre-expanded-tag
Tag: cp37-abi3-manylinux_2_17_x86_64
Tag: cp37-abi3-manylinux2014_x86_64
",
            name = env!("CARGO_PKG_NAME"),
            version = env!("CARGO_PKG_VERSION"),
        );
        let actual = wheel_file(&[
            "py2.py3-none-any".to_string(),
            "pre-expanded-tag".to_string(),
            "cp37-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64".to_string(),
        ])?;
        assert_eq!(expected, actual);

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn add_data_resolves_symlink_targets_and_uses_source_permissions() -> Result<()> {
        let tmp_dir = TempDir::new()?;
        let source = tmp_dir.path().join("README.md");
        fs::write(&source, b"hello from symlink target")?;
        fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644))?;

        let data_dir = tmp_dir.path().join("test-pkg.data");
        let linked_dir = data_dir.join("data/data");
        fs::create_dir_all(&linked_dir)?;
        symlink("../../../README.md", linked_dir.join("README.md"))?;

        let metadata = Metadata24::new("test-pkg".to_string(), Version::new([1, 0]));
        let wheel_dir = tmp_dir.path().join("dist");
        fs::create_dir_all(&wheel_dir)?;

        let wheel_writer = WheelWriter::new(
            "py3-none-any",
            &wheel_dir,
            &metadata,
            SimpleFileOptions::default(),
        )?;
        let mut writer = VirtualWriter::new(wheel_writer, Override::empty());
        let wheel_path = {
            add_data(&mut writer, &metadata, Some(&data_dir))?;
            writer.finish(&metadata, tmp_dir.path(), &["py3-none-any".to_string()])?
        };

        let mut wheel = ZipArchive::new(fs::File::open(&wheel_path)?)?;
        let entry_name = metadata
            .get_data_dir()
            .join("data/data/README.md")
            .to_string_lossy()
            .replace('\\', "/");
        {
            let mut entry = wheel.by_name(&entry_name)?;
            assert_eq!(entry.unix_mode().map(|mode| mode & 0o777), Some(0o644));

            let mut content = String::new();
            entry.read_to_string(&mut content)?;
            assert_eq!(content, "hello from symlink target");
        }

        let record_name = metadata
            .get_dist_info_dir()
            .join("RECORD")
            .to_string_lossy()
            .replace('\\', "/");
        assert!(wheel.by_name(&record_name).is_ok());

        tmp_dir.close()?;
        Ok(())
    }
}
