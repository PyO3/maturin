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
use crate::archive_source::ArchiveSource;
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
    write_dist_info(
        &mut writer,
        &context.project_layout.project_root,
        &context.metadata24,
        &context.tags_from_bridge().unwrap(),
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
