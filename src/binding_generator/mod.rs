use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;

use crate::Metadata24;
use crate::ModuleWriter;
use crate::module_writer::ModuleWriterExt;

mod cffi_binding;
mod pyo3_binding;
mod uniffi_binding;
mod wasm_binding;

pub use cffi_binding::write_cffi_module;
pub use pyo3_binding::write_bindings_module;
pub use uniffi_binding::write_uniffi_module;
pub use wasm_binding::write_wasm_launcher;

/// Adds a data directory with a scripts directory with the binary inside it
pub fn write_bin(
    writer: &mut impl ModuleWriter,
    artifact: &Path,
    metadata: &Metadata24,
    bin_name: &str,
) -> Result<()> {
    let data_dir = PathBuf::from(format!(
        "{}-{}.data",
        &metadata.get_distribution_escaped(),
        &metadata.version
    ))
    .join("scripts");

    // We can't use add_file since we need to mark the file as executable
    writer.add_file(data_dir.join(bin_name), artifact, true)?;
    Ok(())
}
