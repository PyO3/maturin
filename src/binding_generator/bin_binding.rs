use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr as _;

use anyhow::Context;
use anyhow::Result;
use pep508_rs::Requirement;

use crate::BuildArtifact;
use crate::BuildContext;
use crate::Metadata24;
use crate::archive_source::ArchiveSource;
use crate::archive_source::GeneratedSourceData;

use super::BindingGenerator;
use super::GeneratorOutput;

/// A generator for producing bin (and wasm) bindings.
pub struct BinBindingGenerator<'m> {
    metadata: &'m mut Metadata24,
}

impl<'m> BinBindingGenerator<'m> {
    pub fn new(metadata: &'m mut Metadata24) -> Self {
        Self { metadata }
    }
}

impl<'m> BindingGenerator for BinBindingGenerator<'m> {
    fn generate_bindings(
        &mut self,
        context: &BuildContext,
        artifact: &BuildArtifact,
        _module: &Path,
    ) -> Result<GeneratorOutput> {
        // I wouldn't know of any case where this would be the wrong (and neither do
        // I know a better alternative)
        let bin_name = artifact
            .path
            .file_name()
            .context("Couldn't get the filename from the binary produced by cargo")?
            .to_str()
            .context("binary produced by cargo has non-utf8 filename")?
            .to_string();
        let scripts_dir = self.metadata.get_data_dir().join("scripts");
        let artifact_target = scripts_dir.join(&bin_name);

        let mut additional_files = None;
        if context.target.is_wasi() {
            update_entry_points(self.metadata, &bin_name)?;

            let mut files = HashMap::new();
            files.insert(
                Path::new(&self.metadata.get_distribution_escaped())
                    .join(bin_name.replace('-', "_"))
                    .with_extension("py"),
                ArchiveSource::Generated(GeneratedSourceData {
                    data: generate_wasm_launcher(&bin_name).into(),
                    path: None,
                    executable: false,
                }),
            );
            additional_files = Some(files);
        }

        Ok(GeneratorOutput {
            artifact_target,
            artifact_source_override: None,
            additional_files,
        })
    }
}

/// Adds a wrapper script that starts the wasm binary through wasmtime.
pub fn generate_wasm_launcher(bin_name: &str) -> String {
    format!(
        r#"from pathlib import Path

from wasmtime import Store, Module, Engine, WasiConfig, Linker

import sysconfig

def main():
    # The actual executable
    program_location = Path(sysconfig.get_path("scripts")).joinpath("{bin_name}")
    # wasmtime-py boilerplate
    engine = Engine()
    store = Store(engine)
    # TODO: is there an option to just get the default of the wasmtime cli here?
    wasi = WasiConfig()
    wasi.inherit_argv()
    wasi.inherit_env()
    wasi.inherit_stdout()
    wasi.inherit_stderr()
    wasi.inherit_stdin()
    # TODO: Find a real solution here. Maybe there's an always allow callback?
    # Even fancier would be something configurable in pyproject.toml
    wasi.preopen_dir(".", ".")
    store.set_wasi(wasi)
    linker = Linker(engine)
    linker.define_wasi()
    module = Module.from_file(store.engine, str(program_location))
    linking1 = linker.instantiate(store, module)
    # TODO: this is taken from https://docs.wasmtime.dev/api/wasmtime/struct.Linker.html#method.get_default
    #       is this always correct?
    start = linking1.exports(store).get("") or linking1.exports(store)["_start"]
    start(store)

if __name__ == '__main__':
    main()
    "#
    )
}

/// Insert wasm launcher scripts as entrypoints and the wasmtime dependency
fn update_entry_points(metadata24: &mut Metadata24, bin_name: &str) -> Result<()> {
    let distribution_name = metadata24.get_distribution_escaped();
    let console_scripts = metadata24
        .entry_points
        .entry("console_scripts".to_string())
        .or_default();

    // From https://packaging.python.org/en/latest/specifications/entry-points/
    // > The name may contain any characters except =, but it cannot start or end with any
    // > whitespace character, or start with [. For new entry points, it is recommended to
    // > use only letters, numbers, underscores, dots and dashes (regex [\w.-]+).
    // All of these rules are already enforced by cargo:
    // https://github.com/rust-lang/cargo/blob/58a961314437258065e23cb6316dfc121d96fb71/src/cargo/util/restricted_names.rs#L39-L84
    // i.e. we don't need to do any bin name validation here anymore
    let base_name = bin_name
        .strip_suffix(".wasm")
        .context("No .wasm suffix in wasi binary")?;
    console_scripts.insert(
        base_name.to_string(),
        format!("{distribution_name}.{}:main", base_name.replace('-', "_")),
    );

    // Add our wasmtime default version if the user didn't provide one
    if !metadata24
        .requires_dist
        .iter()
        .any(|requirement| requirement.name.as_ref() == "wasmtime")
    {
        // Having the wasmtime version hardcoded is not ideal, it's easy enough to overwrite
        metadata24
            .requires_dist
            .push(Requirement::from_str("wasmtime>=11.0.0,<12.0.0").unwrap());
    }

    Ok(())
}
