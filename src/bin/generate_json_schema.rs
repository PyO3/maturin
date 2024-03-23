use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use schemars::schema_for;

use maturin::pyproject_toml::ToolMaturin;

pub(crate) fn main() -> Result<()> {
    let schema = schema_for!(ToolMaturin);
    let schema_string = serde_json::to_string_pretty(&schema).unwrap();
    let filename = "maturin.schema.json";
    let schema_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(filename);

    let current = fs::read_to_string(&schema_path)?;
    if current == schema_string {
        println!("Up-to-date: {filename}");
    } else {
        println!("Updating: {filename}");
        fs::write(schema_path, schema_string.as_bytes())?;
    }

    Ok(())
}
