use std::io::Error as IoError;
use std::io::Write;

use anyhow::Result;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::Digest as _;
use sha2::Sha256;

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
