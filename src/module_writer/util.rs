use std::collections::HashMap;
use std::io::Error as IoError;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use anyhow::bail;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use same_file::is_same_file;
use sha2::Digest as _;
use sha2::Sha256;

/// Keep track of which files we added from where, so we can skip duplicate files and error when
/// adding conflicting files.
///
/// The wrapped type contains as key the path added to the archive and as value the originating path
/// on the file system or `None` for generated files.
#[derive(Default)]
pub(super) struct FileTracker {
    pub files: HashMap<PathBuf, Option<PathBuf>>,
}

impl FileTracker {
    /// Returns `true` if the file should be added, `false` if an identical file was already added
    /// (skip) and an error if a different file was already added.
    pub(super) fn add_file(&mut self, target: &Path, source: Option<&Path>) -> Result<bool> {
        let Some(previous_source) = self
            .files
            .insert(target.to_path_buf(), source.map(|path| path.to_path_buf()))
        else {
            // The path doesn't exist in the archive yet.
            return Ok(true);
        };
        match (previous_source, source) {
            (None, None) => {
                bail!(
                    "Generated file {} was already added, can't add it again",
                    target.display()
                );
            }
            (Some(previous_source), None) => {
                bail!(
                    "File {} was already added from {}, can't overwrite with generated file",
                    target.display(),
                    previous_source.display()
                )
            }
            (None, Some(source)) => {
                bail!(
                    "Generated file {} was already added, can't overwrite it with {}",
                    target.display(),
                    source.display()
                );
            }
            (Some(previous_source), Some(source)) => {
                if is_same_file(source, &previous_source).unwrap_or(false) {
                    // Ignore identical duplicate files
                    Ok(false)
                } else {
                    bail!(
                        "File {} was already added from {}, can't add it from {}",
                        target.display(),
                        previous_source.display(),
                        source.display()
                    );
                }
            }
        }
    }
}

pub(super) struct StreamSha256<'a, W> {
    hasher: Sha256,
    inner: &'a mut W,
    bytes_written: usize,
}

impl<'a, W> StreamSha256<'a, W>
where
    W: Write,
{
    pub(super) fn new(inner: &'a mut W) -> Self {
        Self {
            hasher: Sha256::new(),
            inner,
            bytes_written: 0,
        }
    }

    pub(super) fn finalize(self) -> Result<(String, usize)> {
        self.inner.flush()?;
        let hash = URL_SAFE_NO_PAD.encode(self.hasher.finalize());
        Ok((hash, self.bytes_written))
    }
}

impl<'a, W> Write for StreamSha256<'a, W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        self.bytes_written += written;
        Ok(written)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.inner.flush()
    }
}
