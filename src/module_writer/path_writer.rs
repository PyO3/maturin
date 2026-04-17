use std::io;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use fs_err as fs;
use fs_err::File;
#[cfg(unix)]
use fs_err::OpenOptions;
#[cfg(unix)]
use fs_err::os::unix::fs::OpenOptionsExt as _;

use crate::archive_source::ArchiveSource;

use super::ModuleWriterInternal;
#[cfg(target_family = "unix")]
use super::default_permission;

/// A [ModuleWriter] that adds the module somewhere in the filesystem, e.g. in a virtualenv
pub struct PathWriter {
    base_path: PathBuf,
}

impl super::private::Sealed for PathWriter {}

impl ModuleWriterInternal for PathWriter {
    fn add_entry(&mut self, target: impl AsRef<Path>, source: ArchiveSource) -> Result<()> {
        let target = self.base_path.join(target);
        if let Some(parent_dir) = target.parent() {
            fs::create_dir_all(parent_dir)
                .with_context(|| format!("Failed to create directory {parent_dir:?}"))?;
        }

        // We only need to set the executable bit on unix
        let mut file = {
            #[cfg(target_family = "unix")]
            {
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .mode(default_permission(source.executable()))
                    .open(&target)
            }
            #[cfg(target_os = "windows")]
            {
                File::create(&target)
            }
        }
        .with_context(|| format!("Failed to create a file at {target:?}"))?;

        match source {
            ArchiveSource::Generated(source) => io::copy(&mut source.data.as_slice(), &mut file),
            ArchiveSource::File(source) => {
                let mut source_file =
                    File::options()
                        .read(true)
                        .open(&source.path)
                        .with_context(|| {
                            format!("Failed to open file at {:?} for reading", source.path)
                        })?;

                io::copy(&mut source_file, &mut file)
            }
        }
        .context("Failed to copy entry to target")?;

        Ok(())
    }
}

impl PathWriter {
    /// Writes the module to the given path
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        Self {
            base_path: path.as_ref().to_path_buf(),
        }
    }
}
