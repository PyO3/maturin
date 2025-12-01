use std::collections::BTreeMap;
use std::io;
use std::io::Read;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use fs_err::File;
use ignore::overrides::Override;
use normpath::PathExt as _;
use tracing::debug;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::Metadata24;
use crate::project_layout::ProjectLayout;

use super::ModuleWriter;
use super::default_permission;
use super::util::FileTracker;
use super::util::StreamSha256;
use super::write_dist_info;

/// A glorified zip builder, mostly useful for writing the record file of a wheel
pub struct WheelWriter {
    zip: ZipWriter<File>,
    record: BTreeMap<PathBuf, (String, usize)>,
    file_tracker: FileTracker,
    excludes: Override,
    file_options: SimpleFileOptions,
    target_exclusion_warning_emitted: bool,
}

impl ModuleWriter for WheelWriter {
    fn add_bytes(
        &mut self,
        target: impl AsRef<Path>,
        source: Option<&Path>,
        mut data: impl Read,
        executable: bool,
    ) -> Result<()> {
        if let Some(source) = source {
            if self.exclude(source) {
                return Ok(());
            }
        }

        let target = target.as_ref();
        if self.exclude(target) {
            if !self.target_exclusion_warning_emitted {
                self.target_exclusion_warning_emitted = true;
                eprintln!(
                    "⚠️ Warning: A file was excluded from the archive by the target path in the archive\n\
                     ⚠️ instead of the source path on the filesystem. This behavior is deprecated and\n\
                     ⚠️ will be removed in future versions of maturin.",
                );
            }
            debug!("Excluded file {target:?} from archive by target path");
            return Ok(());
        }

        if !self.file_tracker.add_file(target, source)? {
            // Ignore duplicate files.
            return Ok(());
        }

        let options = self
            .file_options
            .unix_permissions(default_permission(executable));
        self.zip.start_file_from_path(target, options)?;
        let mut writer = StreamSha256::new(&mut self.zip);

        io::copy(&mut data, &mut writer)
            .with_context(|| format!("Failed to write to zip archive for {target:?}"))?;

        let (hash, length) = writer.finalize()?;
        self.record.insert(target.to_path_buf(), (hash, length));

        Ok(())
    }
}

impl WheelWriter {
    /// Create a new wheel file which can be subsequently expanded
    ///
    /// Adds the .dist-info directory and the METADATA file in it
    pub fn new(
        tag: &str,
        wheel_dir: &Path,
        metadata24: &Metadata24,
        excludes: Override,
        file_options: SimpleFileOptions,
    ) -> Result<WheelWriter> {
        let wheel_path = wheel_dir.join(format!(
            "{}-{}-{}.whl",
            metadata24.get_distribution_escaped(),
            metadata24.get_version_escaped(),
            tag
        ));

        let file = File::create(wheel_path)?;

        let builder = WheelWriter {
            zip: ZipWriter::new(file),
            record: BTreeMap::new(),
            file_tracker: FileTracker::default(),
            excludes,
            file_options,
            target_exclusion_warning_emitted: false,
        };

        Ok(builder)
    }

    /// Add a pth file to wheel root for editable installs
    pub fn add_pth(
        &mut self,
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
                self.add_bytes(target, None, python_path.as_bytes(), false)?;
            } else {
                eprintln!(
                    "⚠️ source code path contains non-Unicode sequences, editable installs may not work."
                );
            }
        }
        Ok(())
    }

    /// Returns `true` if the given path should be excluded
    fn exclude(&self, path: impl AsRef<Path>) -> bool {
        self.excludes.matched(path.as_ref(), false).is_whitelist()
    }

    /// Creates the record file and finishes the zip
    pub fn finish(
        mut self,
        metadata24: &Metadata24,
        pyproject_dir: &Path,
        tags: &[String],
    ) -> Result<PathBuf> {
        write_dist_info(&mut self, pyproject_dir, metadata24, tags)?;

        let options = self
            .file_options
            .unix_permissions(default_permission(false));
        let record_filename = metadata24.get_dist_info_dir().join("RECORD");
        debug!("Adding {}", record_filename.display());
        self.zip.start_file_from_path(&record_filename, options)?;

        for (filename, (hash, len)) in self.record {
            let filename = filename.to_string_lossy();
            writeln!(self.zip, "{filename},sha256={hash},{len}")?;
        }
        // Write the record for the RECORD file itself
        writeln!(self.zip, "{},,", record_filename.display())?;

        let file = self.zip.finish()?;
        Ok(file.into_path())
    }
}

#[cfg(test)]
mod tests {
    use ignore::overrides::Override;
    use pep440_rs::Version;
    use tempfile::TempDir;

    use crate::CompressionMethod;
    use crate::CompressionOptions;
    use crate::Metadata24;

    use super::WheelWriter;

    #[test]
    fn wheel_writer_no_compression() -> Result<(), Box<dyn std::error::Error>> {
        let metadata = Metadata24::new("dummy".to_string(), Version::new([1, 0]));
        let tmp_dir = TempDir::new()?;
        let compression_options = CompressionOptions {
            compression_method: CompressionMethod::Stored,
            ..Default::default()
        };

        let writer = WheelWriter::new(
            "no compression",
            tmp_dir.path(),
            &metadata,
            Override::empty(),
            compression_options.get_file_options(),
        )?;

        writer.finish(&metadata, tmp_dir.path(), &[])?;
        tmp_dir.close()?;

        Ok(())
    }
}
