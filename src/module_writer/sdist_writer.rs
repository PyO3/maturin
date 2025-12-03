use std::io;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use flate2::Compression;
use flate2::write::GzEncoder;
use fs_err as fs;
use fs_err::File;
use ignore::overrides::Override;
use normpath::PathExt as _;
use tracing::debug;

use crate::Metadata24;
use crate::archive_source::ArchiveSource;

use super::ModuleWriterInternal;
use super::default_permission;
use super::util::FileTracker;

/// A deterministic, arbitrary, non-zero timestamp that use used as `mtime`
/// of headers when writing sdists.
///
/// This value, copied from the tar crate, corresponds to _Jul 23, 2006_,
/// which is the date of the first commit for what would become Rust.
///
/// This value is used instead of unix epoch 0 because some tools do not handle
/// the 0 value properly (See rust-lang/cargo#9512).
const SDIST_DETERMINISTIC_TIMESTAMP: u64 = 1153704088;

/// Creates a .tar.gz archive containing the source distribution
pub struct SDistWriter {
    tar: tar::Builder<GzEncoder<Vec<u8>>>,
    path: PathBuf,
    file_tracker: FileTracker,
    excludes: Override,
    mtime: u64,
    target_exclusion_warning_emitted: bool,
}

impl super::private::Sealed for SDistWriter {}

impl ModuleWriterInternal for SDistWriter {
    fn add_entry(&mut self, target: impl AsRef<Path>, source: ArchiveSource) -> Result<()> {
        let source_path = source.path();
        if let Some(source) = source_path {
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

        if !self.file_tracker.add_file(target, source_path)? {
            // Ignore duplicate files.
            return Ok(());
        }

        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(default_permission(source.executable()));
        header.set_mtime(self.mtime);

        let data = match source {
            ArchiveSource::Generated(source) => source.data,
            ArchiveSource::File(source) => {
                let mut file =
                    File::options()
                        .read(true)
                        .open(&source.path)
                        .with_context(|| {
                            format!("Failed to open file {:?} for reading", source.path)
                        })?;
                let mut buffer = Vec::new();
                file.read_to_end(&mut buffer)
                    .context("Failed to read file into buffer")?;
                buffer
            }
        };

        header.set_size(data.len() as u64);

        self.tar
            .append_data(&mut header, target, data.as_slice())
            .with_context(|| {
                format!(
                    "Failed to add {} bytes to sdist as {}",
                    data.len(),
                    target.display()
                )
            })?;
        Ok(())
    }
}

impl SDistWriter {
    /// Create a source distribution .tar.gz which can be subsequently expanded
    pub fn new(
        wheel_dir: impl AsRef<Path>,
        metadata24: &Metadata24,
        excludes: Override,
        mtime_override: Option<u64>,
    ) -> Result<Self, io::Error> {
        let path = wheel_dir
            .as_ref()
            .normalize()?
            .join(format!(
                "{}-{}.tar.gz",
                &metadata24.get_distribution_escaped(),
                &metadata24.get_version_escaped()
            ))
            .into_path_buf();

        let enc = GzEncoder::new(Vec::new(), Compression::default());
        let mut tar = tar::Builder::new(enc);
        tar.mode(tar::HeaderMode::Deterministic);

        Ok(Self {
            tar,
            path,
            file_tracker: FileTracker::default(),
            excludes,
            mtime: mtime_override.unwrap_or(SDIST_DETERMINISTIC_TIMESTAMP),
            target_exclusion_warning_emitted: false,
        })
    }

    /// Returns `true` if the given path should be excluded
    fn exclude(&self, path: impl AsRef<Path>) -> bool {
        self.excludes.matched(path.as_ref(), false).is_whitelist()
    }

    /// Finished the .tar.gz archive
    pub fn finish(self) -> Result<PathBuf, io::Error> {
        let archive = self.tar.into_inner()?;
        fs::write(&self.path, archive.finish()?)?;
        Ok(self.path)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ignore::overrides::Override;
    use ignore::overrides::OverrideBuilder;
    use pep440_rs::Version;
    use tempfile::TempDir;

    use crate::Metadata24;
    use crate::ModuleWriter;
    use crate::module_writer::EMPTY;

    use super::SDistWriter;

    #[test]
    // The mechanism is the same for wheel_writer
    fn sdist_writer_excludes() -> Result<(), Box<dyn std::error::Error>> {
        let metadata = Metadata24::new("dummy".to_string(), Version::new([1, 0]));

        // No excludes
        let tmp_dir = TempDir::new()?;
        let mut writer = SDistWriter::new(&tmp_dir, &metadata, Override::empty(), None)?;
        assert!(writer.file_tracker.files.is_empty());
        writer.add_empty_file("test")?;
        assert_eq!(writer.file_tracker.files.len(), 1);
        writer.finish()?;
        tmp_dir.close()?;

        // A test filter
        let tmp_dir = TempDir::new()?;
        let mut excludes = OverrideBuilder::new(&tmp_dir);
        excludes.add("test*")?;
        excludes.add("!test2")?;
        let mut writer = SDistWriter::new(&tmp_dir, &metadata, excludes.build()?, None)?;
        writer.add_bytes("test1", Some(Path::new("test1")), EMPTY, true)?;
        writer.add_bytes("test3", Some(Path::new("test3")), EMPTY, true)?;
        assert!(writer.file_tracker.files.is_empty());
        writer.add_bytes("test2", Some(Path::new("test2")), EMPTY, true)?;
        assert!(!writer.file_tracker.files.is_empty());
        writer.add_bytes("yes", Some(Path::new("yes")), EMPTY, true)?;
        assert_eq!(writer.file_tracker.files.len(), 2);
        writer.finish()?;
        tmp_dir.close()?;

        Ok(())
    }
}
