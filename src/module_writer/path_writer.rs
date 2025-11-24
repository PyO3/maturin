use std::io;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use fs_err as fs;
#[cfg(target_os = "windows")]
use fs_err::File;
#[cfg(unix)]
use fs_err::OpenOptions;
#[cfg(unix)]
use fs_err::os::unix::fs::OpenOptionsExt as _;

use super::ModuleWriter;
#[cfg(target_family = "unix")]
use super::default_permission;
use super::util::FileTracker;

/// A [ModuleWriter] that adds the module somewhere in the filesystem, e.g. in a virtualenv
pub struct PathWriter {
    base_path: PathBuf,
    file_tracker: FileTracker,
}

impl PathWriter {
    /// Writes the module to the given path
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        Self {
            base_path: path.as_ref().to_path_buf(),
            file_tracker: FileTracker::default(),
        }
    }
}

impl ModuleWriter for PathWriter {
    fn add_bytes(
        &mut self,
        target: impl AsRef<Path>,
        source: Option<&Path>,
        mut data: impl Read,
        #[cfg_attr(target_os = "windows", allow(unused_variables))] executable: bool,
    ) -> Result<()> {
        let path = self.base_path.join(&target);

        if !self.file_tracker.add_file(target.as_ref(), source)? {
            // Ignore duplicate files.
            return Ok(());
        }

        if let Some(parent_dir) = path.parent() {
            fs::create_dir_all(parent_dir)
                .with_context(|| format!("Failed to create directory {}", parent_dir.display()))?;
        }

        // We only need to set the executable bit on unix
        let mut file = {
            #[cfg(target_family = "unix")]
            {
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .mode(default_permission(executable))
                    .open(&path)
            }
            #[cfg(target_os = "windows")]
            {
                File::create(&path)
            }
        }
        .with_context(|| format!("Failed to create a file at {}", path.display()))?;

        io::copy(&mut data, &mut file)
            .with_context(|| format!("Failed to write to file at {}", path.display()))?;

        Ok(())
    }
}
