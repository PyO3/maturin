#![cfg(feature = "schemars")]

use fs_err as fs;
use std::path::PathBuf;

use anyhow::{Result, bail};
use pretty_assertions::StrComparison;
use schemars::schema_for;

use crate::pyproject_toml::ToolMaturin;

#[derive(Debug, Copy, Clone, PartialEq, Eq, clap::ValueEnum, Default)]
/// The mode to use when generating the JSON schema.
pub enum Mode {
    /// Write the JSON schema to the file.
    #[default]
    Write,
    /// Check if the JSON schema is up-to-date.
    Check,
    /// Print the JSON schema to stdout.
    DryRun,
}

/// Generate the JSON schema for the `pyproject.toml` file.
#[derive(Debug, clap::Parser)]
pub struct GenerateJsonSchemaOptions {
    /// The mode to use when generating the JSON schema.
    #[arg(long, default_value_t, value_enum)]
    pub mode: Mode,
}

/// Generate the JSON schema for the `pyproject.toml` file.
pub fn generate_json_schema(args: GenerateJsonSchemaOptions) -> Result<()> {
    let schema = schema_for!(ToolMaturin);
    let schema_string = serde_json::to_string_pretty(&schema).unwrap();
    let filename = "maturin.schema.json";
    let schema_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(filename);

    match args.mode {
        Mode::DryRun => {
            println!("{schema_string}");
        }
        Mode::Check => {
            let current = fs::read_to_string(schema_path)?;
            if current == schema_string {
                println!("Up-to-date: {filename}");
            } else {
                let comparison = StrComparison::new(&current, &schema_string);
                bail!(
                    "{filename} changed, please run `cargo run --features schemars -- generate-json-schema`:\n{comparison}",
                );
            }
        }
        Mode::Write => {
            let current = fs::read_to_string(&schema_path)?;
            if current == schema_string {
                println!("Up-to-date: {filename}");
            } else {
                println!("Updating: {filename}");
                fs::write(schema_path, schema_string.as_bytes())?;
            }
        }
    }

    Ok(())
}
