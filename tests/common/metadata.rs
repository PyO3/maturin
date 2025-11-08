use anyhow::Result;
use insta::assert_snapshot;
use maturin::{BuildOptions, CargoOptions, ModuleWriter, write_dist_info};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct MockWriter {
    files: Vec<String>,
    contents: HashMap<String, String>,
}

impl ModuleWriter for MockWriter {
    fn add_data(
        &mut self,
        target: impl AsRef<Path>,
        _source: Option<&Path>,
        mut data: impl std::io::Read,
        _executable: bool,
    ) -> Result<()> {
        let target = target.as_ref().to_string_lossy().to_string();
        let mut buffer = String::new();
        data.read_to_string(&mut buffer)?;

        self.files.push(target.clone());
        self.contents.insert(target, buffer);

        Ok(())
    }
}

#[test]
fn metadata_hello_world_pep639() {
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

    let mut writer = MockWriter::default();
    write_dist_info(
        &mut writer,
        &context.project_layout.project_root,
        &context.metadata24,
        &context.tags_from_bridge().unwrap(),
    )
    .unwrap();

    assert_snapshot!(writer.files.join("\n").replace("\\", "/"), @r"
    hello_world-0.1.0.dist-info/METADATA
    hello_world-0.1.0.dist-info/WHEEL
    hello_world-0.1.0.dist-info/licenses/LICENSE
    hello_world-0.1.0.dist-info/licenses/licenses/AUTHORS.txt
    ");
    let metadata_path = Path::new("hello_world-0.1.0.dist-info")
        .join("METADATA")
        .to_str()
        .unwrap()
        .to_string();
    // Remove the README in the body of the email
    let metadata = writer.contents[&metadata_path]
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
}
