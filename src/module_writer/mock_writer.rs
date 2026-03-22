use std::io::Read as _;
use std::path::Path;
use std::path::PathBuf;
use std::str;

use anyhow::Result;
use anyhow::bail;
use fs_err::File;
use ignore::overrides::Override;
use indexmap::IndexMap;
use indexmap::map::Entry;
use insta::assert_snapshot;
use itertools::Itertools as _;

use crate::BuildOptions;
use crate::CargoOptions;
use crate::Metadata24;
use crate::archive_source::ArchiveSource;
use crate::build_orchestrator::BuildOrchestrator;
use crate::write_dist_info;

use super::ModuleWriterInternal;
use super::VirtualWriter;

#[derive(Default)]
pub(crate) struct MockWriter {
    files: IndexMap<PathBuf, Vec<u8>>,
}

impl super::private::Sealed for MockWriter {}

impl ModuleWriterInternal for MockWriter {
    fn add_entry(&mut self, target: impl AsRef<Path>, source: ArchiveSource) -> Result<()> {
        let target = target.as_ref().to_path_buf();
        let Entry::Vacant(entry) = self.files.entry(target.clone()) else {
            bail!("Duplicate file {target:?} written to mock writer");
        };

        let buffer = match source {
            ArchiveSource::Generated(source) => source.data,
            ArchiveSource::File(source) => {
                let mut file = File::options().read(true).open(source.path)?;
                let mut buffer = Vec::new();
                file.read_to_end(&mut buffer)?;
                buffer
            }
        };

        entry.insert(buffer);
        Ok(())
    }
}

impl MockWriter {
    pub fn finish(self) -> IndexMap<PathBuf, Vec<u8>> {
        self.files
    }
}

#[test]
fn metadata_hello_world_pep639() -> Result<()> {
    let build_options = BuildOptions {
        cargo: CargoOptions {
            manifest_path: Some(
                PathBuf::from("test-crates")
                    .join("hello-world")
                    .join("Cargo.toml"),
            ),
            ..CargoOptions::default()
        },
        ..BuildOptions::default()
    };
    let context = build_options.into_build_context().build().unwrap();

    let mut writer = VirtualWriter::new(MockWriter::default(), Override::empty());
    let orchestrator = BuildOrchestrator::new(&context);
    write_dist_info(
        &mut writer,
        &context.project.project_layout.project_root,
        &context.project.metadata24,
        &orchestrator.tags_from_bridge().unwrap(),
    )
    .unwrap();

    let files = writer.finish()?;
    assert_snapshot!(files.keys().map(|p| p.to_string_lossy()).collect_vec().join("\n").replace("\\", "/"), @r"
    hello_world-0.1.0.dist-info/METADATA
    hello_world-0.1.0.dist-info/WHEEL
    hello_world-0.1.0.dist-info/licenses/LICENSE
    hello_world-0.1.0.dist-info/licenses/licenses/AUTHORS.txt
    ");
    let metadata_path = Path::new("hello_world-0.1.0.dist-info").join("METADATA");
    // Remove the README in the body of the email
    let metadata = str::from_utf8(&files[&metadata_path])
        .unwrap()
        .split_once("\n\n")
        .unwrap()
        .0;
    assert_snapshot!(metadata, @r"
    Metadata-Version: 2.4
    Name: hello-world
    Version: 0.1.0
    License-File: LICENSE
    License-File: licenses/AUTHORS.txt
    Author: konstin <konstin@mailbox.org>
    Author-email: konstin <konstin@mailbox.org>
    Description-Content-Type: text/markdown; charset=UTF-8; variant=GFM
    ");

    Ok(())
}

#[test]
fn write_dist_info_uses_license_file_sources() -> Result<()> {
    use pep440_rs::Version;
    use std::str::FromStr;

    let temp_dir = tempfile::tempdir()?;
    let pyproject_dir = temp_dir.path().join("crate");
    let workspace_root = temp_dir.path();
    fs_err::create_dir_all(&pyproject_dir)?;

    // Create a workspace-level license file (outside pyproject_dir)
    let license_content = b"MIT License - workspace level";
    fs_err::write(workspace_root.join("LICENSE"), license_content)?;

    let mut metadata = Metadata24::new("test-pkg".to_string(), Version::from_str("1.0.0").unwrap());
    metadata.license_files.push(PathBuf::from("LICENSE"));
    metadata
        .license_file_sources
        .insert(PathBuf::from("LICENSE"), workspace_root.join("LICENSE"));

    let mut writer = VirtualWriter::new(MockWriter::default(), Override::empty());
    write_dist_info(
        &mut writer,
        &pyproject_dir,
        &metadata,
        &["py3-none-any".to_string()],
    )?;

    let files = writer.finish()?;
    let license_key = Path::new("test_pkg-1.0.0.dist-info/licenses/LICENSE");
    assert!(
        files.contains_key(license_key),
        "expected license file in dist-info, got keys: {:?}",
        files.keys().collect::<Vec<_>>()
    );
    assert_eq!(files[license_key], license_content);

    Ok(())
}

#[test]
#[serial_test::serial]
fn write_dist_info_rejects_absolute_license_paths() {
    use pep440_rs::Version;
    use std::str::FromStr;

    let temp_dir = tempfile::tempdir().unwrap();
    let pyproject_dir = temp_dir.path();

    let mut metadata = Metadata24::new("test-pkg".to_string(), Version::from_str("1.0.0").unwrap());
    metadata.license_files.push(temp_dir.path().join("LICENSE"));

    let mut writer = VirtualWriter::new(MockWriter::default(), Override::empty());
    let err = write_dist_info(
        &mut writer,
        pyproject_dir,
        &metadata,
        &["py3-none-any".to_string()],
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("unsafe path"),
        "unexpected error: {err:#}"
    );
}

#[test]
#[serial_test::serial]
fn write_dist_info_respects_metadata_directory_env_var() -> Result<()> {
    use pep440_rs::Version;
    use std::str::FromStr;

    let temp_dir = tempfile::tempdir()?;
    let pyproject_dir = temp_dir.path().join("crate");
    fs_err::create_dir_all(&pyproject_dir)?;

    let metadata = Metadata24::new("test-pkg".to_string(), Version::from_str("1.0.0").unwrap());
    let dist_info_name = "test_pkg-1.0.0.dist-info";

    // Create a pre-generated .dist-info directory with custom METADATA
    let metadata_dir = temp_dir.path().join("metadata");
    let pre_existing_dir = metadata_dir.join(dist_info_name);
    fs_err::create_dir_all(&pre_existing_dir)?;

    let custom_metadata =
        "Metadata-Version: 2.4\nName: test-pkg\nVersion: 1.0.0\nClassifier: Custom :: Classifier\n";
    fs_err::write(pre_existing_dir.join("METADATA"), custom_metadata)?;
    fs_err::write(pre_existing_dir.join("WHEEL"), "should be overwritten")?;
    fs_err::write(pre_existing_dir.join("RECORD"), "should be skipped")?;
    fs_err::write(
        pre_existing_dir.join("entry_points.txt"),
        "[console_scripts]\nfoo=bar:main\n",
    )?;

    // Create a licenses/ subdirectory
    let licenses_dir = pre_existing_dir.join("licenses");
    fs_err::create_dir_all(&licenses_dir)?;
    fs_err::write(licenses_dir.join("LICENSE"), "MIT License")?;

    // Per PEP 517, metadata_directory points to the .dist-info directory itself
    // SAFETY: This test is serialized and the env var is removed before returning.
    unsafe { std::env::set_var("MATURIN_PEP517_METADATA_DIR", &pre_existing_dir) };
    let mut writer = VirtualWriter::new(MockWriter::default(), Override::empty());
    let tags = &["cp310-cp310-manylinux_2_17_x86_64".to_string()];
    let result = write_dist_info(&mut writer, &pyproject_dir, &metadata, tags);
    unsafe { std::env::remove_var("MATURIN_PEP517_METADATA_DIR") };
    result?;

    let files = writer.finish()?;

    // METADATA should be the custom one, not regenerated
    let metadata_key = Path::new(dist_info_name).join("METADATA");
    assert_eq!(
        str::from_utf8(&files[&metadata_key]).unwrap(),
        custom_metadata
    );

    // WHEEL should be regenerated with correct tags, not the pre-existing content
    let wheel_key = Path::new(dist_info_name).join("WHEEL");
    let wheel_content = str::from_utf8(&files[&wheel_key]).unwrap();
    assert!(
        wheel_content.contains("Tag: cp310-cp310-manylinux_2_17_x86_64"),
        "WHEEL should contain the correct tag, got: {wheel_content}"
    );
    assert!(
        !wheel_content.contains("should be overwritten"),
        "WHEEL should be regenerated, not copied"
    );

    // RECORD should not be present (it's generated later by WheelWriter)
    let record_key = Path::new(dist_info_name).join("RECORD");
    assert!(!files.contains_key(&record_key));

    // entry_points.txt should be copied
    let ep_key = Path::new(dist_info_name).join("entry_points.txt");
    assert_eq!(
        str::from_utf8(&files[&ep_key]).unwrap(),
        "[console_scripts]\nfoo=bar:main\n"
    );

    // License file from subdirectory should be copied
    let license_key = Path::new(dist_info_name).join("licenses").join("LICENSE");
    assert_eq!(str::from_utf8(&files[&license_key]).unwrap(), "MIT License");

    Ok(())
}

#[test]
#[serial_test::serial]
fn write_dist_info_metadata_dir_as_parent_directory() -> Result<()> {
    use pep440_rs::Version;
    use std::str::FromStr;

    let temp_dir = tempfile::tempdir()?;
    let pyproject_dir = temp_dir.path().join("crate");
    fs_err::create_dir_all(&pyproject_dir)?;

    let metadata = Metadata24::new("test-pkg".to_string(), Version::from_str("1.0.0").unwrap());
    let dist_info_name = "test_pkg-1.0.0.dist-info";

    // Create a parent directory containing the .dist-info subdirectory
    let parent_dir = temp_dir.path().join("metadata");
    let pre_existing_dir = parent_dir.join(dist_info_name);
    fs_err::create_dir_all(&pre_existing_dir)?;

    let custom_metadata =
        "Metadata-Version: 2.4\nName: test-pkg\nVersion: 1.0.0\nClassifier: Custom :: Classifier\n";
    fs_err::write(pre_existing_dir.join("METADATA"), custom_metadata)?;
    fs_err::write(pre_existing_dir.join("WHEEL"), "should be overwritten")?;

    // Set env var to the parent directory (PEP 517 spec form)
    // SAFETY: This test is serialized and the env var is removed before returning.
    unsafe { std::env::set_var("MATURIN_PEP517_METADATA_DIR", &parent_dir) };
    let mut writer = VirtualWriter::new(MockWriter::default(), Override::empty());
    let tags = &["cp310-cp310-manylinux_2_17_x86_64".to_string()];
    let result = write_dist_info(&mut writer, &pyproject_dir, &metadata, tags);
    unsafe { std::env::remove_var("MATURIN_PEP517_METADATA_DIR") };
    result?;

    let files = writer.finish()?;

    // METADATA should be the custom one
    let metadata_key = Path::new(dist_info_name).join("METADATA");
    assert_eq!(
        str::from_utf8(&files[&metadata_key]).unwrap(),
        custom_metadata
    );

    // WHEEL should be regenerated
    let wheel_key = Path::new(dist_info_name).join("WHEEL");
    let wheel_content = str::from_utf8(&files[&wheel_key]).unwrap();
    assert!(
        wheel_content.contains("Tag: cp310-cp310-manylinux_2_17_x86_64"),
        "WHEEL should contain the correct tag, got: {wheel_content}"
    );

    Ok(())
}
