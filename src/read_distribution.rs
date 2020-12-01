use anyhow::{bail, Context, Result};
use fs_err::File;
use mailparse::parse_mail;
use std::io::{BufReader, Read};
use std::path::Path;
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
    let name = format!("{}.dist-info/METADATA", dist_name_version);
    let mut metadata_email = Vec::new();
    archive
        .by_name(&name)
        .context(format!(
            "This wheel does not contain a METADATA file at {}, which is mandatory for wheels",
            name
        ))?
        .read_to_end(&mut metadata_email)
        .context("Failed to read METADATA file")?;

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

/// Returns the metadata for a source distribution (.tar.gz).
/// Only parses the filename since dist-info is not part of source
/// distributions
fn read_metadata_for_source_distribution(path: impl AsRef<Path>) -> Result<Vec<(String, String)>> {
    let filename = filename_from_file(&path)?
        .strip_suffix(".tar.gz")
        // We checked that before entering this function
        .unwrap()
        .to_string();
    let metadata = match filename.split('-').collect::<Vec<_>>().as_slice() {
        [name, version] => vec![
            ("Metadata-Version".to_string(), "2.1".to_string()),
            ("Name".to_string(), name.to_string()),
            ("Version".to_string(), version.to_string()),
        ],
        _ => bail!("Invalid wheel name: {}", filename),
    };
    Ok(metadata)
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
        assert_eq!(
            metadata,
            vec![
                ("Metadata-Version".to_string(), "2.1".to_string()),
                ("Name".to_string(), "pyo3_mixed".to_string()),
                ("Version".to_string(), "2.1.1".to_string())
            ]
        );
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
        assert!(metadata.clone()[7].1.starts_with("# pyo3-mixed"));
        assert!(metadata.clone()[7].1.ends_with("tox.ini\n\n"));
    }
}
