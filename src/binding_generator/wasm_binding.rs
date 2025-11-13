use std::path::Path;

use anyhow::Result;

use crate::Metadata24;
use crate::ModuleWriter;

/// Adds a wrapper script that start the wasm binary through wasmtime.
///
/// Note that the wasm binary needs to be written separately by [write_bin]
pub fn write_wasm_launcher(
    writer: &mut impl ModuleWriter,
    metadata: &Metadata24,
    bin_name: &str,
) -> Result<()> {
    let entrypoint_script = format!(
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
    );

    let launcher_path = Path::new(&metadata.get_distribution_escaped())
        .join(bin_name.replace('-', "_"))
        .with_extension("py");
    writer.add_bytes(&launcher_path, None, entrypoint_script.as_bytes(), true)?;
    Ok(())
}
