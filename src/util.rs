use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;
use zip::DateTime;

use fs_err as fs;

pub(crate) fn sha256_hex(hash: &[u8]) -> String {
    use std::fmt::Write;

    let mut hex = String::with_capacity(hash.len() * 2);
    for b in hash {
        write!(hex, "{b:02x}").unwrap();
    }
    hex
}

/// Wrapper that implements [`io::Write`] for any [`Digest`] hasher,
/// restoring the `io::copy` pattern removed in digest 0.11.
pub(crate) struct DigestWriter<D>(pub D);

impl<D: Digest> io::Write for DigestWriter<D> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Calculate the sha256 of a file
pub(crate) fn hash_file(path: impl AsRef<Path>) -> Result<String, io::Error> {
    let mut file = fs::File::open(path.as_ref())?;
    let mut w = DigestWriter(Sha256::new());
    io::copy(&mut file, &mut w)?;
    Ok(sha256_hex(w.0.finalize().as_slice()))
}

/// Returns a DateTime representing the value SOURCE_DATE_EPOCH environment variable
/// Note that the earliest timestamp a zip file can represent is 1980-01-01
pub(crate) fn zip_mtime() -> DateTime {
    let res: anyhow::Result<DateTime> = (|| {
        let epoch: i64 = std::env::var("SOURCE_DATE_EPOCH")?.parse()?;
        let dt = time::OffsetDateTime::from_unix_timestamp(epoch)?;
        let dt = time::PrimitiveDateTime::new(dt.date(), dt.time());
        let dt = DateTime::try_from(dt)?;
        Ok(dt)
    })();

    res.unwrap_or_default()
}
