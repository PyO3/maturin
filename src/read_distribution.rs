use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use fs_err::File;
use mailparse::parse_mail;
use regex::Regex;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

fn filename_from_file(path: impl AsRef<Path>) -> Result<String> {
    Ok(path
        .as_ref()
        .file_name()
        .context("Missing filename")?
        .to_str()
        .context("Expected a utf-8 filename")?
        .to_string())
}

/// Read the email format into key value pairs
fn metadata_from_bytes(metadata_email: &mut Vec<u8>) -> Result<Vec<(String, String)>> {
    let metadata_email = parse_mail(&metadata_email).context("Failed to parse METADATA")?;

    let mut metadata = Vec::new();
    for header in &metadata_email.headers {
        metadata.push((header.get_key().to_string(), header.get_value().to_string()));
    }

    let body = metadata_email
        .get_body()
        .context("Failed to parse METADATA")?;
    if !body.trim().is_empty() {
        metadata.push(("Description".into(), body));
    }
    Ok(metadata)
}

/// Port of pip's `canonicalize_name`
/// https://github.com/pypa/pip/blob/b33e791742570215f15663410c3ed987d2253d5b/src/pip/_vendor/packaging/utils.py#L18-L25
fn canonicalize_name(name: &str) -> String {
    Regex::new("[-_.]+")
        .unwrap()
        .replace(name, "-")
        .to_lowercase()
}

/// Reads the METADATA file in the .dist-info directory of a wheel, returning
/// the metadata (https://packaging.python.org/specifications/core-metadata/)
/// as key value pairs
fn read_metadata_for_wheel(path: impl AsRef<Path>) -> Result<Vec<(String, String)>> {
    let filename = filename_from_file(&path)?
        .strip_suffix(".whl")
        // We checked that before entering this function
        .unwrap()
        .to_string();
    let parts: Vec<_> = filename.split('-').collect();
    let dist_name_version = match parts.as_slice() {
        [name, version, _python_tag, _abi_tag, _platform_tag] => format!("{}-{}", name, version),
        _ => bail!("The wheel name is invalid: {}", filename),
    };
    let reader = BufReader::new(File::open(path.as_ref())?);
    let mut archive = ZipArchive::new(reader).context("Failed to read file as zip")?;
    // The METADATA format is an email (RFC 822)
    // pip's implementation: https://github.com/pypa/pip/blob/b33e791742570215f15663410c3ed987d2253d5b/src/pip/_internal/utils/wheel.py#L109-L144
    // twine's implementation: https://github.com/pypa/twine/blob/534385596820129b41cbcdcc83d34aa8788067f1/twine/wheel.py#L52-L56
    // We mostly follow pip
    let name = format!("{}.dist-info/METADATA", dist_name_version);
    let mut metadata_email = Vec::new();

    // Find the metadata file
    let metadata_files: Vec<_> = archive
        .file_names()
        .filter(|i| canonicalize_name(i) == canonicalize_name(&name))
        .map(ToString::to_string)
        .collect();

    match &metadata_files.as_slice() {
        [] => bail!(
            "This wheel does not contain a METADATA matching {}, which is mandatory for wheels",
            name
        ),
        [metadata_file] => archive
            .by_name(&metadata_file)
            .context(format!("Failed to read METADATA file {}", metadata_file))?
            .read_to_end(&mut metadata_email)
            .context(format!("Failed to read METADATA file {}", metadata_file))?,
        files => bail!(
            "Found more than one metadata file matching {}: {:?}",
            name,
            files
        ),
    };

    metadata_from_bytes(&mut metadata_email)
}

/// Returns the metadata for a source distribution (.tar.gz).
/// Only parses the filename since dist-info is not part of source
/// distributions
fn read_metadata_for_source_distribution(path: impl AsRef<Path>) -> Result<Vec<(String, String)>> {
    let mut reader = tar::Archive::new(GzDecoder::new(BufReader::new(File::open(path.as_ref())?)));
    // Unlike for wheels, in source distributions the metadata is stored in a file called PKG-INFO
    // try_find would be ideal here, but it's nightly only
    let mut entry = reader
        .entries()?
        .map(|entry| -> Result<_> {
            let entry = entry?;
            if entry.path()? == PathBuf::from("PKG-INFO") {
                Ok(Some(entry))
            } else {
                Ok(None)
            }
        })
        .find_map(|x| x.transpose())
        .context(format!(
            "Source distribution {:?} does not contain a PKG-INFO, but it should",
            path.as_ref()
        ))?
        .context(format!("Failed to read {:?}", path.as_ref()))?;
    let mut metadata_email = Vec::new();
    entry
        .read_to_end(&mut metadata_email)
        .context(format!("Failed to read {:?}", path.as_ref()))?;
    metadata_from_bytes(&mut metadata_email)
}

/// Returns the metadata as key value pairs for a wheel or a source distribution
pub fn get_metadata_for_distribution(path: &Path) -> Result<Vec<(String, String)>> {
    let filename = filename_from_file(path)?;
    if filename.ends_with(".whl") {
        read_metadata_for_wheel(path)
            .context(format!("Failed to read metadata from wheel at {:?}", path))
    } else if filename.ends_with(".tar.gz") {
        read_metadata_for_source_distribution(path).context(format!(
            "Failed to read metadata from source distribution at {:?}",
            path
        ))
    } else {
        bail!("File has an unknown extension: {:?}", path)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_source_distribution() {
        let metadata =
            get_metadata_for_distribution(Path::new("test-data/pyo3_mixed-2.1.1.tar.gz")).unwrap();
        let expected: Vec<_> = [
            ("Metadata-Version", "2.1"),
            ("Name", "pyo3-mixed"),
            ("Version", "2.1.1"),
            ("Summary", "Implements a dummy function combining rust and python"),
            ("Author", "konstin <konstin@mailbox.org>"),
            ("Author-Email", "konstin <konstin@mailbox.org>"),
            ("Description-Content-Type", "text/markdown; charset=UTF-8; variant=GFM"),
            ("Description", "# pyo3-mixed\n\nA package for testing maturin with a mixed pyo3/python project.\n\n"),
        ].iter().map(|(k,v)| (k.to_string(), v.to_string())).collect();

        assert_eq!(metadata, expected);
    }

    #[test]
    fn test_wheel() {
        let metadata = get_metadata_for_distribution(Path::new(
            "test-data/pyo3_mixed-2.1.1-cp38-cp38-manylinux1_x86_64.whl",
        ))
        .unwrap();
        assert_eq!(
            metadata.iter().map(|x| &x.0).collect::<Vec::<&String>>(),
            vec![
                "Metadata-Version",
                "Name",
                "Version",
                "Summary",
                "Author",
                "Author-Email",
                "Description-Content-Type",
                "Description"
            ]
        );
        // Check the description
        assert!(metadata[7].1.starts_with("# pyo3-mixed"));
        assert!(metadata[7].1.ends_with("tox.ini\n\n"));
    }
}
