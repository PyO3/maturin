use std::fmt::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
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
use tracing::debug;

use crate::Metadata24;
use crate::PyProjectToml;
use crate::archive_source::ArchiveSource;
use crate::archive_source::FileSourceData;
use crate::archive_source::GeneratedSourceData;
use crate::project_layout::ProjectLayout;
use crate::pyproject_toml::Format;

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

const EMPTY: Vec<u8> = vec![];

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
        self.add_bytes(target, None, EMPTY, false)
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
        let absolute = absolute?.into_path();
        if !python_packages
            .iter()
            .any(|path| absolute.starts_with(path))
        {
            continue;
        }
        let relative = absolute.strip_prefix(python_dir).unwrap();
        if !absolute.is_dir() {
            // Ignore native libraries from develop, if any
            if let Some(extension) = relative.extension() {
                if extension.to_string_lossy() == "so" {
                    debug!("Ignoring native library {}", relative.display());
                    continue;
                }
            }
            #[cfg(unix)]
            let mode = absolute.metadata()?.permissions().mode();
            #[cfg(not(unix))]
            let mode = 0o644;
            writer
                .add_file(relative, &absolute, permission_is_executable(mode))
                .context(format!("File to add file from {}", absolute.display()))?;
        }
    }

    // Include additional files
    if let Some(pyproject) = pyproject_toml {
        // FIXME: in src-layout pyproject.toml isn't located directly in python dir
        let pyproject_dir = python_dir;
        if let Some(glob_patterns) = pyproject.include() {
            for pattern in glob_patterns
                .iter()
                .filter_map(|glob_pattern| glob_pattern.targets(Format::Wheel))
            {
                eprintln!("📦 Including files matching \"{pattern}\"");
                for source in glob::glob(&pyproject_dir.join(pattern).to_string_lossy())
                    .with_context(|| format!("Invalid glob pattern: {pattern}"))?
                    .filter_map(Result::ok)
                {
                    let target = source.strip_prefix(pyproject_dir)?.to_path_buf();
                    if !source.is_dir() {
                        #[cfg(unix)]
                        let mode = source.metadata()?.permissions().mode();
                        #[cfg(not(unix))]
                        let mode = 0o644;
                        writer.add_file(target, source, permission_is_executable(mode))?;
                    }
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
            (|| {
                for file in WalkBuilder::new(subdir.path())
                    .standard_filters(false)
                    .build()
                {
                    let file = file?;
                    #[cfg(unix)]
                    let mode = file.metadata()?.permissions().mode();
                    #[cfg(not(unix))]
                    let mode = 0o644;
                    let relative = metadata24
                        .get_data_dir()
                        .join(file.path().strip_prefix(data).unwrap());

                    if file.path_is_symlink() {
                        // Copy the actual file contents, not the link, so that you can create a
                        // data directory by joining different data sources
                        let source = fs::read_link(file.path())?;
                        writer.add_file(
                            relative,
                            source.parent().unwrap(),
                            permission_is_executable(mode),
                        )?;
                    } else if file.path().is_file() {
                        writer.add_file(relative, file.path(), permission_is_executable(mode))?;
                    } else if file.path().is_dir() {
                        // Intentionally ignored
                    } else {
                        bail!("Can't handle data dir entry {}", file.path().display());
                    }
                }
                Ok(())
            })()
            .with_context(|| format!("Failed to include data from {}", data.display()))?
        }
    }
    Ok(())
}

/// Creates the .dist-info directory and fills it with all metadata files except RECORD
pub fn write_dist_info(
    writer: &mut VirtualWriter<impl ModuleWriterInternal>,
    pyproject_dir: &Path,
    metadata24: &Metadata24,
    tags: &[String],
) -> Result<PathBuf> {
    let dist_info_dir = metadata24.get_dist_info_dir();

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
            writer.add_file(
                license_files_dir.join(path),
                pyproject_dir.join(path),
                false,
            )?;
        }
    }

    Ok(dist_info_dir)
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
fn permission_is_executable(mode: u32) -> bool {
    (0o100 & mode) == 0o100
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
    use super::wheel_file;

    #[test]
    fn wheel_file_compressed_tags() -> Result<(), Box<dyn std::error::Error>> {
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
}
