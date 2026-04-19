use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr as _;

use anyhow::Context;
use anyhow::Result;
use path_slash::PathBufExt as _;
use pep508_rs::Requirement;

use crate::BuildArtifact;
use crate::BuildContext;
use crate::Metadata24;
use crate::archive_source::ArchiveSource;
use crate::archive_source::GeneratedSourceData;
use crate::binding_generator::ArtifactTarget;

use super::BindingGenerator;
use super::GeneratorOutput;

/// A generator for producing bin (and wasm) bindings.
pub struct BinBindingGenerator<'m> {
    metadata: &'m mut Metadata24,
    /// When `true`, the real binary is placed in `{dist_name}.scripts/` in
    /// platlib and a Python shim is placed in `.data/scripts/` instead.
    /// This is needed when the binary has external shared library
    /// dependencies that are bundled into the wheel's `.libs/` directory,
    /// because the relative path from the installed `bin/` to
    /// `site-packages/{dist}.libs/` is unpredictable across installations.
    use_shim: bool,
}

impl<'m> BinBindingGenerator<'m> {
    pub fn new(metadata: &'m mut Metadata24, use_shim: bool) -> Self {
        Self { metadata, use_shim }
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

        let mut additional_files = None;

        let artifact_target = if self.use_shim {
            // The real binary goes into platlib at {dist}.scripts/{bin_name}
            // so it has a predictable relative path to {dist}.libs/.
            // A Python shim in .data/scripts/ execs the real binary at runtime.
            let real_bin_dir = self.metadata.get_scripts_platlib_dir();
            let shim_path = real_bin_dir.join(&bin_name);
            let shim_path_str = shim_path.to_slash_lossy();
            let libs_dir_name = self.metadata.get_distribution_escaped() + ".libs";

            // On Windows, bin_name includes `.exe`. The shim script must NOT
            // have an `.exe` extension — otherwise Windows treats it as a PE
            // binary and pip skips wrapper generation. Strip the extension for
            // the shim entry in .data/scripts/.
            let shim_name = bin_name.strip_suffix(".exe").unwrap_or(&bin_name);

            let mut files = HashMap::new();
            files.insert(
                scripts_dir.join(shim_name),
                ArchiveSource::Generated(GeneratedSourceData {
                    data: generate_script_shim(&shim_path_str, &libs_dir_name).into(),
                    path: None,
                    executable: true,
                }),
            );
            additional_files = Some(files);
            ArtifactTarget::Binary(real_bin_dir.join(&bin_name))
        } else {
            ArtifactTarget::Binary(scripts_dir.join(&bin_name))
        };

        if context.project.target.is_wasi() {
            update_entry_points(self.metadata, &bin_name)?;

            let dist_name = self.metadata.get_distribution_escaped();
            let files = additional_files.get_or_insert_with(HashMap::new);
            files.insert(
                Path::new(&dist_name)
                    .join(bin_name.replace('-', "_"))
                    .with_extension("py"),
                ArchiveSource::Generated(GeneratedSourceData {
                    data: generate_wasm_launcher(&bin_name).into(),
                    path: None,
                    executable: false,
                }),
            );
        }

        Ok(GeneratorOutput {
            artifact_target,
            artifact_source_override: None,
            additional_files,
        })
    }
}

/// Generate a Python shim script that execs the real binary from platlib.
///
/// This is used when a bin-type wheel has external shared library dependencies
/// that are bundled into `{dist}.libs/` in platlib. The real binary is placed
/// in `{dist}.scripts/` (also in platlib) so it has a predictable RPATH to
/// the libs directory. The shim replaces the original script entry and uses
/// `sysconfig.get_path("platlib")` to locate the real binary at runtime.
///
/// On Windows, `os.add_dll_directory()` is called to register the `.libs/`
/// directory for DLL search before exec'ing the binary, since Windows does
/// not use RPATH/`@loader_path` for DLL resolution.
///
/// This approach matches [auditwheel's handling](https://github.com/pypa/auditwheel/pull/443).
fn generate_script_shim(binary_path: &str, libs_dir_name: &str) -> String {
    format!(
        "#!python\n\
         import os\n\
         import sys\n\
         import sysconfig\n\
         \n\
         \n\
         if __name__ == \"__main__\":\n\
         \x20   platlib = sysconfig.get_path(\"platlib\")\n\
         \x20   exe = os.path.join(platlib, \"{binary_path}\")\n\
         \x20   libs_dir = os.path.join(platlib, \"{libs_dir_name}\")\n\
         \x20   if sys.platform == \"win32\" and os.path.isdir(libs_dir):\n\
         \x20       os.add_dll_directory(libs_dir)\n\
         \x20       os.environ[\"PATH\"] = libs_dir + os.pathsep + os.environ.get(\"PATH\", \"\")\n\
         \x20   os.execv(exe, [exe] + sys.argv[1:])\n"
    )
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
