use std::cmp::Ordering;
use std::io;
use std::io::Read;
use std::io::Write as _;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::{Result, bail};
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
    pax_header_index: u64,
}

impl super::private::Sealed for SDistWriter {}

impl ModuleWriterInternal for SDistWriter {
    fn add_entry(&mut self, target: impl AsRef<Path>, source: ArchiveSource) -> Result<()> {
        let target = target.as_ref();
        let archive_path = portable_path(target)?;

        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(default_permission(source.executable()));
        set_sdist_header_metadata(&mut header, self.mtime);

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

        if set_ustar_path_or_needs_pax(&mut header, &archive_path)? {
            self.append_pax_path(&archive_path)?;
            header.set_path(format!("PaxHeaders/{}", self.pax_header_index))?;
            self.pax_header_index += 1;
        }
        header.set_cksum();

        self.tar.append(&header, data.as_slice()).with_context(|| {
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
            pax_header_index: 0,
        })
    }

    fn append_pax_path(&mut self, archive_path: &str) -> Result<(), io::Error> {
        let data = pax_record("path", archive_path.as_bytes())?;

        let mut header = tar::Header::new_ustar();
        header.set_path("././@PaxHeader")?;
        header.set_entry_type(tar::EntryType::XHeader);
        header.set_mode(0o644);
        set_sdist_header_metadata(&mut header, self.mtime);
        header.set_size(data.len() as u64);
        header.set_cksum();

        self.tar.append(&header, data.as_slice())
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

fn set_sdist_header_metadata(header: &mut tar::Header, mtime: u64) {
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(mtime);
}

/// Avoid the extra size for the path in the pax extended header if not required.
///
/// This matches CPython:
/// <https://github.com/python/cpython/blob/8ab7b43a14bed4780febbd7586a41cfe459aa6d5/Lib/tarfile.py#L1069-L1125>
fn set_ustar_path_or_needs_pax(
    header: &mut tar::Header,
    archive_path: &str,
) -> Result<bool, io::Error> {
    // ustar only allows ASCII, but would accept arbitrary bytes without the check.
    if !archive_path.is_ascii() {
        return Ok(true);
    }

    let mut probe = tar::Header::new_ustar();
    if probe.set_path(archive_path).is_err() {
        return Ok(true);
    }

    header.set_path(archive_path)?;
    Ok(false)
}

/// Convert the path to a UTF-8 string with forward slashes and no NUL bytes.
fn portable_path(path: &Path) -> Result<String> {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::CurDir => None,
            component => Some(component),
        })
        .map(|component| {
            let Component::Normal(component) = component else {
                bail!("sdist archive paths must be relative and must not contain `..`");
            };

            let component = component.to_str().with_context(|| {
                format!("sdist archive path `{}` is not valid UTF-8", path.display())
            })?;
            if component.contains('\0') {
                bail!(
                    "sdist archive path `{}` contains a NUL byte",
                    path.display()
                );
            }
            Ok(component)
        })
        .collect::<Result<Vec<_>>>()?;

    if components.is_empty() {
        bail!("sdist archive paths must not be empty");
    }

    Ok(components.join("/"))
}

/// Format in the pax header `LEN KEY=VALUE\n` format.
fn pax_record(key: &str, value: &[u8]) -> Result<Vec<u8>, io::Error> {
    // POSIX pax records are `LEN KEY=VALUE\n`, and `LEN` includes its own
    // decimal digits. Start with a one-digit length and grow at powers of ten.
    let mut digits_len = 1;
    let mut max_len = 10;
    let key_value_len = " ".len() + key.len() + "=".len() + value.len() + "\n".len();
    while digits_len + key_value_len >= max_len {
        // We require space for an additional digit in the length field, which increases the total
        // possible length ten-fold.
        digits_len += 1;
        max_len *= 10;
    }

    let len = digits_len + key_value_len;
    let mut data = Vec::with_capacity(len);
    write!(&mut data, "{len} {key}=")?;
    data.extend_from_slice(value);
    data.push(b'\n');
    Ok(data)
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::path::Path;

    use expect_test::expect;
    use flate2::read::GzDecoder;
    use fs_err as fs;
    use pep440_rs::Version;
    use tempfile::TempDir;

    use crate::Metadata24;
    use crate::archive_source::ArchiveSource;
    use crate::archive_source::GeneratedSourceData;

    use super::ModuleWriterInternal;
    use super::SDistWriter;

    #[test]
    fn sdist_uses_pax_header_for_extended_path() -> anyhow::Result<()> {
        let temp_dir = TempDir::new()?;
        let metadata = Metadata24::new("test-pkg".to_string(), Version::new([1, 0]));
        let mut writer = SDistWriter::new(temp_dir.path(), &metadata, None)?;

        let long_path = format!("data/{}.txt", "a".repeat(101));
        writer.add_entry(
            &long_path,
            ArchiveSource::Generated(GeneratedSourceData {
                data: b"test\n".to_vec(),
                path: None,
                executable: false,
            }),
        )?;
        let sdist = writer.finish()?;

        let [pax_header, pax_data, file_header] = raw_tar_blocks(&sdist)?;
        expect![[r#"
            block 0 bytes 0..157:
            @PaxHeader\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00
            \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00
            \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00
            \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00
            \x00\x00\x00\x000000644\x000000000\x000000
            000\x0000000000170\x0010461020
            230\x000007762\x00x"#]].assert_eq(&pax_header);
        expect![[r#"
            block 1 bytes 0..128:
            120 path=data/aaaaaaaaaa
            aaaaaaaaaaaaaaaaaaaaaaaa
            aaaaaaaaaaaaaaaaaaaaaaaa
            aaaaaaaaaaaaaaaaaaaaaaaa
            aaaaaaaaaaaaaaaaaaa.txt\n
            \x00\x00\x00\x00\x00\x00\x00\x00"#]]
        .assert_eq(&pax_data);
        expect![[r#"
            block 2 bytes 0..157:
            PaxHeaders/0\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00
            \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00
            \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00
            \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00
            \x00\x00\x00\x000000644\x000000000\x000000
            000\x0000000000005\x0010461020
            230\x000010071\x000"#]].assert_eq(&file_header);

        Ok(())
    }

    fn raw_tar_blocks(sdist_path: &Path) -> anyhow::Result<[String; 3]> {
        let tar_gz = fs::File::open(sdist_path)?;
        let mut decoder = GzDecoder::new(tar_gz);
        let mut archive = Vec::new();
        decoder.read_to_end(&mut archive)?;

        Ok([
            raw_tar_block_slice(&archive, 0, 0..157)?,
            raw_tar_block_slice(&archive, 1, 0..128)?,
            raw_tar_block_slice(&archive, 2, 0..157)?,
        ])
    }

    fn raw_tar_block_slice(
        archive: &[u8],
        block: usize,
        range: std::ops::Range<usize>,
    ) -> anyhow::Result<String> {
        let start_in_block = range.start;
        let end_in_block = range.end;
        let start = block * 512 + start_in_block;
        let end = block * 512 + end_in_block;
        let Some(bytes) = archive.get(start..end) else {
            anyhow::bail!("missing tar block {block} bytes {start_in_block}..{end_in_block}");
        };

        let mut output = format!("block {block} bytes {start_in_block}..{end_in_block}:");
        for chunk in bytes.chunks(24) {
            output.push('\n');
            output.push_str(&chunk.escape_ascii().to_string());
        }
        Ok(output)
    }
}
