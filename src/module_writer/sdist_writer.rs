use std::cmp::Ordering;
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
use normpath::PathExt as _;

use crate::Metadata24;
use crate::archive_source::ArchiveSource;

use super::ModuleWriterInternal;
use super::default_permission;

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
    mtime: u64,
}

impl super::private::Sealed for SDistWriter {}

impl ModuleWriterInternal for SDistWriter {
    fn add_entry(&mut self, target: impl AsRef<Path>, source: ArchiveSource) -> Result<()> {
        let target = target.as_ref();

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
            mtime: mtime_override.unwrap_or(SDIST_DETERMINISTIC_TIMESTAMP),
        })
    }

    /// Tar files do not have a central directory of entries, so the entire file needs to be walked
    /// to find a specific entry. Most tools are interested in the package metadata, so place that
    /// at the beginning of the tar for convenience
    pub(super) fn file_ordering<'a>(
        &self,
        pkg_info_path: &'a Path,
    ) -> impl FnMut(&PathBuf, &PathBuf) -> Ordering + use<'a> {
        move |p1, p2| {
            let p1_is_info = p1 == pkg_info_path;
            let p2_is_info = p2 == pkg_info_path;

            match (p1_is_info, p2_is_info) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => p1.cmp(p2),
            }
        }
    }

    /// Finished the .tar.gz archive
    pub fn finish(self) -> Result<PathBuf, io::Error> {
        let archive = self.tar.into_inner()?;
        fs::write(&self.path, archive.finish()?)?;
        Ok(self.path)
    }
}
