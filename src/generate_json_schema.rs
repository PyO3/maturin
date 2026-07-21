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
    let mut schema_string = serde_json::to_string_pretty(&schema).unwrap();
    // `serde_json::to_string_pretty` doesn't emit a trailing newline, but the
    // checked-in schema file always has one (added by the pre-commit
    // end-of-file-fixer hook). Normalize here so all three modes agree on
    // the same string.
    schema_string.push('\n');
    let filename = "maturin.schema.json";
    let schema_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(filename);

    match args.mode {
        Mode::DryRun => {
            print!("{schema_string}");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `Mode::Check` must succeed on a clean checkout. This exercises the
    /// actual normalization behavior inside `generate_json_schema` (rather
    /// than a copy of it), so it will catch a regression where the
    /// trailing-newline fix is dropped or altered and `check` starts
    /// failing again.
    #[test]
    fn test_check_mode_succeeds_on_clean_checkout() {
        generate_json_schema(GenerateJsonSchemaOptions { mode: Mode::Check })
            .expect("check mode must succeed on a clean checkout");
    }

    /// The generated schema string (including its normalized trailing
    /// newline) must match the checked-in `maturin.schema.json` byte for
    /// byte. This additionally pins the schema content itself (e.g. catches
    /// a stale checked-in file), separate from the check-mode behavior
    /// asserted above.
    #[test]
    fn test_generated_schema_matches_checked_in_file() {
        let schema = schema_for!(ToolMaturin);
        let mut schema_string = serde_json::to_string_pretty(&schema).unwrap();
        schema_string.push('\n');

        let schema_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("maturin.schema.json");
        let current = fs::read_to_string(schema_path).unwrap();

        assert_eq!(
            current, schema_string,
            "maturin.schema.json is out of date, please run `cargo run --features schemars -- generate-json-schema`"
        );
        assert!(
            schema_string.ends_with('\n') && !schema_string.ends_with("\n\n"),
            "generated schema should end with exactly one trailing newline"
        );
    }
}
