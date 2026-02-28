use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;
use zip::DateTime;

use fs_err as fs;

/// Calculate the sha256 of a file
pub(crate) fn hash_file(path: impl AsRef<Path>) -> Result<String, io::Error> {
    let mut file = fs::File::open(path.as_ref())?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    let hex = format!("{:x}", hasher.finalize());
    Ok(hex)
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
