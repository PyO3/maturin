use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::io;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use fs_err::File;
use tracing::debug;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::Metadata24;
use crate::archive_source::ArchiveSource;

use super::ModuleWriterInternal;
use super::default_permission;
use super::util::StreamSha256;

/// A glorified zip builder, mostly useful for writing the record file of a wheel
pub struct WheelWriter {
    zip: ZipWriter<File>,
    record: BTreeMap<PathBuf, (String, usize)>,
    file_options: SimpleFileOptions,
}

impl super::private::Sealed for WheelWriter {}

impl ModuleWriterInternal for WheelWriter {
    fn add_entry(&mut self, target: impl AsRef<Path>, source: ArchiveSource) -> Result<()> {
        let target = target.as_ref();
        let options = self
            .file_options
            .unix_permissions(default_permission(source.executable()));
        self.zip.start_file_from_path(target, options)?;
        let mut writer = StreamSha256::new(&mut self.zip);

        match source {
            ArchiveSource::Generated(source) => io::copy(&mut source.data.as_slice(), &mut writer),
            ArchiveSource::File(source) => {
                let mut file = File::options()
                    .read(true)
                    .open(&source.path)
                    .with_context(|| format!("Failed to open file {:?}", source.path))?;

                io::copy(&mut file, &mut writer)
            }
        }
        .with_context(|| format!("Failed to write to zip archive for {target:?}"))?;

        let (hash, length) = writer.finalize()?;
        self.record.insert(target.to_path_buf(), (hash, length));

        Ok(())
    }
}

impl WheelWriter {
    /// Create a new wheel file which can be subsequently expanded
    pub fn new(
        tag: &str,
        wheel_dir: &Path,
        metadata24: &Metadata24,
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
            file_options,
        };

        Ok(builder)
    }

    /// PEP 427 recommends that the .dist-info directory be placed physically at the
    /// end of the zip file, so this custom comparator does exactly that
    pub(super) fn file_ordering<'p>(
        &self,
        dist_info_dir: &'p Path,
    ) -> impl FnMut(&PathBuf, &PathBuf) -> Ordering + use<'p> {
        move |p1, p2| {
            let p1_is_dist_info = p1.starts_with(dist_info_dir);
            let p2_is_dist_info = p2.starts_with(dist_info_dir);
            match (p1_is_dist_info, p2_is_dist_info) {
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                _ => p1.cmp(p2),
            }
        }
    }

    /// Creates the record file and finishes the zip
    pub fn finish(mut self, dist_info_dir: &Path) -> Result<PathBuf> {
        let options = self
            .file_options
            .unix_permissions(default_permission(false));
        let record_filename = dist_info_dir.join("RECORD");
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
            compression_options.get_file_options(),
        )?;

        writer.finish(&metadata.get_dist_info_dir())?;
        tmp_dir.close()?;

        Ok(())
    }
}
