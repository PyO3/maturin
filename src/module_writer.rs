//! The wheel format is (mostly) specified in PEP 427
use crate::project_layout::ProjectLayout;
use crate::{BridgeModel, Metadata21, PythonInterpreter, Target};
use anyhow::{anyhow, bail, Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use fs_err as fs;
use fs_err::File;
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt::Write as _;
#[cfg(target_family = "unix")]
use std::fs::OpenOptions;
use std::io;
use std::io::{Read, Write};
#[cfg(target_family = "unix")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::str;
use tempfile::{tempdir, TempDir};
use tracing::debug;
use zip::{self, ZipWriter};

/// Allows writing the module to a wheel or add it directly to the virtualenv
pub trait ModuleWriter {
    /// Adds a directory relative to the module base path
    fn add_directory(&mut self, path: impl AsRef<Path>) -> Result<()>;

    /// Adds a file with bytes as content in target relative to the module base path
    fn add_bytes(&mut self, target: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
        // 0o644 is the default from the zip crate
        self.add_bytes_with_permissions(target, bytes, 0o644)
    }

    /// Adds a file with bytes as content in target relative to the module base path while setting
    /// the given unix permissions
    fn add_bytes_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        bytes: &[u8],
        permissions: u32,
    ) -> Result<()>;

    /// Copies the source file to the target path relative to the module base path
    fn add_file(&mut self, target: impl AsRef<Path>, source: impl AsRef<Path>) -> Result<()> {
        let read_failed_context = format!("Failed to read {}", source.as_ref().display());
        let mut file = File::open(source.as_ref()).context(read_failed_context.clone())?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).context(read_failed_context)?;
        self.add_bytes(&target, &buffer)
            .context(format!("Failed to write to {}", target.as_ref().display()))?;
        Ok(())
    }

    /// Copies the source file the the target path relative to the module base path while setting
    /// the given unix permissions
    fn add_file_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        source: impl AsRef<Path>,
        permissions: u32,
    ) -> Result<()> {
        let read_failed_context = format!("Failed to read {}", source.as_ref().display());
        let mut file = File::open(source.as_ref()).context(read_failed_context.clone())?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).context(read_failed_context)?;
        self.add_bytes_with_permissions(target.as_ref(), &buffer, permissions)
            .context(format!("Failed to write to {}", target.as_ref().display()))?;
        Ok(())
    }
}

/// A [ModuleWriter] that adds the module somewhere in the filesystem, e.g. in a virtualenv
pub struct PathWriter {
    base_path: PathBuf,
    record: Vec<(String, String, usize)>,
}

impl PathWriter {
    /// Creates a [ModuleWriter] that adds the module to the current virtualenv
    pub fn venv(target: &Target, venv_dir: &Path, bridge: &BridgeModel) -> Result<Self> {
        let interpreter =
            PythonInterpreter::check_executable(target.get_venv_python(&venv_dir), target, bridge)?
                .ok_or_else(|| {
                    anyhow!("Expected `python` to be a python interpreter inside a virtualenv ಠ_ಠ")
                })?;

        let base_path = target.get_venv_site_package(venv_dir, &interpreter);

        Ok(PathWriter {
            base_path,
            record: Vec::new(),
        })
    }

    /// Writes the module to the given path
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        Self {
            base_path: path.as_ref().to_path_buf(),
            record: Vec::new(),
        }
    }

    /// Removes a directory relative to the base path if it exists.
    ///
    /// This is to clean up the contents of an older develop call
    pub fn delete_dir(&self, relative: impl AsRef<Path>) -> Result<()> {
        let absolute = self.base_path.join(relative);
        if absolute.exists() {
            fs::remove_dir_all(&absolute)
                .context(format!("Failed to remove {}", absolute.display()))?;
        }

        Ok(())
    }

    /// Writes the RECORD file after everything else has been written
    pub fn write_record(self, metadata21: &Metadata21) -> Result<()> {
        let record_file = self
            .base_path
            .join(metadata21.get_dist_info_dir())
            .join("RECORD");
        let mut buffer = File::create(&record_file).context(format!(
            "Failed to create a file at {}",
            record_file.display()
        ))?;

        for (filename, hash, len) in self.record {
            buffer
                .write_all(format!("{},sha256={},{}\n", filename, hash, len).as_bytes())
                .context(format!(
                    "Failed to write to file at {}",
                    record_file.display()
                ))?;
        }
        // Write the record for the RECORD file itself
        buffer
            .write_all(format!("{},,\n", record_file.display()).as_bytes())
            .context(format!(
                "Failed to write to file at {}",
                record_file.display()
            ))?;

        Ok(())
    }
}

impl ModuleWriter for PathWriter {
    fn add_directory(&mut self, path: impl AsRef<Path>) -> Result<()> {
        fs::create_dir_all(self.base_path.join(path))?;
        Ok(())
    }

    fn add_bytes_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        bytes: &[u8],
        _permissions: u32,
    ) -> Result<()> {
        let path = self.base_path.join(&target);

        // We only need to set the executable bit on unix
        let mut file = {
            #[cfg(target_family = "unix")]
            {
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .mode(_permissions)
                    .open(&path)
            }
            #[cfg(target_os = "windows")]
            {
                File::create(&path)
            }
        }
        .context(format!("Failed to create a file at {}", path.display()))?;

        file.write_all(bytes)
            .context(format!("Failed to write to file at {}", path.display()))?;

        let hash = base64::encode_config(&Sha256::digest(bytes), base64::URL_SAFE_NO_PAD);
        self.record.push((
            target.as_ref().to_str().unwrap().to_owned(),
            hash,
            bytes.len(),
        ));

        Ok(())
    }
}

/// A glorified zip builder, mostly useful for writing the record file of a wheel
pub struct WheelWriter {
    zip: ZipWriter<File>,
    record: Vec<(String, String, usize)>,
    record_file: PathBuf,
    wheel_path: PathBuf,
}

impl ModuleWriter for WheelWriter {
    fn add_directory(&mut self, _path: impl AsRef<Path>) -> Result<()> {
        Ok(()) // We don't need to create directories in zip archives
    }

    fn add_bytes_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        bytes: &[u8],
        permissions: u32,
    ) -> Result<()> {
        // The zip standard mandates using unix style paths
        let target = target.as_ref().to_str().unwrap().replace('\\', "/");

        // Unlike users which can use the develop subcommand, the tests have to go through
        // packing a zip which pip than has to unpack. This makes this 2-3 times faster
        let compression_method = if cfg!(feature = "faster-tests") {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        let options = zip::write::FileOptions::default()
            .unix_permissions(permissions)
            .compression_method(compression_method);
        self.zip.start_file(target.clone(), options)?;
        self.zip.write_all(bytes)?;

        let hash = base64::encode_config(&Sha256::digest(bytes), base64::URL_SAFE_NO_PAD);
        self.record.push((target, hash, bytes.len()));

        Ok(())
    }
}

impl WheelWriter {
    /// Create a new wheel file which can be subsequently expanded
    ///
    /// Adds the .dist-info directory and the METADATA file in it
    pub fn new(
        tag: &str,
        wheel_dir: &Path,
        metadata21: &Metadata21,
        tags: &[String],
    ) -> Result<WheelWriter> {
        let wheel_path = wheel_dir.join(format!(
            "{}-{}-{}.whl",
            metadata21.get_distribution_escaped(),
            metadata21.get_version_escaped(),
            tag
        ));

        let file = File::create(&wheel_path)?;

        let mut builder = WheelWriter {
            zip: ZipWriter::new(file),
            record: Vec::new(),
            record_file: metadata21.get_dist_info_dir().join("RECORD"),
            wheel_path,
        };

        write_dist_info(&mut builder, metadata21, tags)?;

        Ok(builder)
    }

    /// Add a pth file to wheel root for editable installs
    pub fn add_pth(
        &mut self,
        project_layout: &ProjectLayout,
        metadata21: &Metadata21,
    ) -> Result<()> {
        if let Some(python_module) = &project_layout.python_module {
            let absolute_path = fs::canonicalize(python_module)?;
            if let Some(python_path) = absolute_path.parent().and_then(|p| p.to_str()) {
                let name = metadata21.get_distribution_escaped();
                self.add_bytes(format!("{}.pth", name), python_path.as_bytes())?;
            } else {
                println!("⚠️ source code path contains non-Unicode sequences, editable installs may not work.");
            }
        }
        Ok(())
    }

    /// Creates the record file and finishes the zip
    pub fn finish(mut self) -> Result<PathBuf, io::Error> {
        let compression_method = if cfg!(feature = "faster-tests") {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        let options = zip::write::FileOptions::default().compression_method(compression_method);
        let record_filename = self.record_file.to_str().unwrap().replace('\\', "/");
        self.zip.start_file(&record_filename, options)?;
        for (filename, hash, len) in self.record {
            self.zip
                .write_all(format!("{},sha256={},{}\n", filename, hash, len).as_bytes())?;
        }
        // Write the record for the RECORD file itself
        self.zip
            .write_all(format!("{},,\n", record_filename).as_bytes())?;

        self.zip.finish()?;
        Ok(self.wheel_path)
    }
}

/// Creates a .tar.gz archive containing the source distribution
pub struct SDistWriter {
    tar: tar::Builder<GzEncoder<File>>,
    path: PathBuf,
    files: HashSet<PathBuf>,
}

impl ModuleWriter for SDistWriter {
    fn add_directory(&mut self, _path: impl AsRef<Path>) -> Result<()> {
        Ok(())
    }

    fn add_bytes_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        bytes: &[u8],
        permissions: u32,
    ) -> Result<()> {
        let target = target.as_ref();
        if self.files.contains(target) {
            // Ignore duplicate files
            return Ok(());
        }
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(permissions);
        header.set_cksum();
        self.tar
            .append_data(&mut header, target, bytes)
            .context(format!(
                "Failed to add {} bytes to sdist as {}",
                bytes.len(),
                target.display()
            ))?;
        self.files.insert(target.to_path_buf());
        Ok(())
    }

    fn add_file(&mut self, target: impl AsRef<Path>, source: impl AsRef<Path>) -> Result<()> {
        let source = source.as_ref();
        let target = target.as_ref();
        if source == self.path {
            bail!(
            "Attempting to include the sdist output tarball {} into itself! Check 'cargo package --list' output.",
            source.display()
            );
        }
        if self.files.contains(target) {
            // Ignore duplicate files
            return Ok(());
        }

        self.tar
            .append_path_with_name(source, target)
            .context(format!(
                "Failed to add file from {} to sdist as {}",
                source.display(),
                target.display(),
            ))?;
        self.files.insert(target.to_path_buf());
        Ok(())
    }
}

impl SDistWriter {
    /// Create a source distribution .tar.gz which can be subsequently expanded
    pub fn new(wheel_dir: impl AsRef<Path>, metadata21: &Metadata21) -> Result<Self, io::Error> {
        let path = wheel_dir.as_ref().join(format!(
            "{}-{}.tar.gz",
            &metadata21.get_distribution_escaped(),
            &metadata21.get_version_escaped()
        ));

        let tar_gz = File::create(&path)?;
        let enc = GzEncoder::new(tar_gz, Compression::default());
        let tar = tar::Builder::new(enc);

        Ok(Self {
            tar,
            path,
            files: HashSet::new(),
        })
    }

    /// Finished the .tar.gz archive
    pub fn finish(mut self) -> Result<PathBuf, io::Error> {
        self.tar.finish()?;
        Ok(self.path)
    }
}

fn wheel_file(tags: &[String]) -> Result<String> {
    let mut wheel_file = format!(
        "Wheel-Version: 1.0
Generator: {name} ({version})
Root-Is-Purelib: false
",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
    );

    for tag in tags {
        writeln!(wheel_file, "Tag: {}", tag)?;
    }

    Ok(wheel_file)
}

/// https://packaging.python.org/specifications/entry-points/
fn entry_points_txt(
    entry_type: &str,
    entrypoints: &HashMap<String, String, impl std::hash::BuildHasher>,
) -> String {
    entrypoints
        .iter()
        .fold(format!("[{}]\n", entry_type), |text, (k, v)| {
            text + k + "=" + v + "\n"
        })
}

/// Glue code that exposes `lib`.
fn cffi_init_file() -> &'static str {
    r#"__all__ = ["lib", "ffi"]

import os
from .ffi import ffi

lib = ffi.dlopen(os.path.join(os.path.dirname(__file__), 'native.so'))
del os
"#
}

/// Wraps some boilerplate around error handling when calling python
fn call_python<I, S>(python: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(&python)
        .args(args)
        .output()
        .context(format!("Failed to run python at {:?}", &python))
}

/// Checks if user has provided their own header at `target/header.h`, otherwise
/// we run cbindgen to generate one.
fn cffi_header(crate_dir: &Path, target_dir: &Path, tempdir: &TempDir) -> Result<PathBuf> {
    let maybe_header = target_dir.join("header.h");

    if maybe_header.is_file() {
        println!("💼 Using the existing header at {}", maybe_header.display());
        Ok(maybe_header)
    } else {
        if crate_dir.join("cbindgen.toml").is_file() {
            println!(
                "💼 Using the existing cbindgen.toml configuration. \n\
                 💼 Enforcing the following settings: \n   \
                 - language = \"C\" \n   \
                 - no_includes = true \n   \
                 - no include_guard \t (directives are not yet supported) \n   \
                 - no defines       \t (directives are not yet supported)"
            );
        }

        let mut config = cbindgen::Config::from_root_or_default(&crate_dir);
        config.defines = HashMap::new();
        config.include_guard = None;

        let bindings = cbindgen::Builder::new()
            .with_config(config)
            .with_crate(crate_dir)
            .with_language(cbindgen::Language::C)
            .with_no_includes()
            .generate()
            .context("Failed to run cbindgen")?;

        let header = tempdir.as_ref().join("header.h");
        bindings.write_to_file(&header);
        Ok(header)
    }
}

/// Returns the content of what will become ffi.py by invoking cbindgen and cffi
///
/// Checks if user has provided their own header at `target/header.h`, otherwise
/// we run cbindgen to generate one. Installs cffi if it's missing and we're inside a virtualenv
///
/// We're using the cffi recompiler, which reads the header, translates them into instructions
/// how to load the shared library without the header and then writes those instructions to a
/// file called `ffi.py`. This `ffi.py` will expose an object called `ffi`. This object is used
/// in `__init__.py` to load the shared library into a module called `lib`.
pub fn generate_cffi_declarations(
    crate_dir: &Path,
    target_dir: &Path,
    python: &Path,
) -> Result<String> {
    let tempdir = tempdir()?;
    let header = cffi_header(crate_dir, target_dir, &tempdir)?;

    let ffi_py = tempdir.as_ref().join("ffi.py");

    // Using raw strings is important because on windows there are path like
    // `C:\Users\JohnDoe\AppData\Local\TEmpl\pip-wheel-asdf1234` where the \U
    // would otherwise be a broken unicode escape sequence
    let cffi_invocation = format!(
        r#"
import cffi
from cffi import recompiler

ffi = cffi.FFI()
with open(r"{header}") as header:
    ffi.cdef(header.read())
recompiler.make_py_source(ffi, "ffi", r"{ffi_py}")
"#,
        ffi_py = ffi_py.display(),
        header = header.display(),
    );

    let output = call_python(python, &["-c", &cffi_invocation])?;
    let install_cffi = if !output.status.success() {
        // First, check whether the error was cffi not being installed
        let last_line = str::from_utf8(&output.stderr)?.lines().last().unwrap_or("");
        if last_line == "ModuleNotFoundError: No module named 'cffi'" {
            // Then check whether we're running in a virtualenv.
            // We don't want to modify any global environment
            // https://stackoverflow.com/a/42580137/3549270
            let output = call_python(
                python,
                &["-c", "import sys\nprint(sys.base_prefix != sys.prefix)"],
            )?;

            match str::from_utf8(&output.stdout)?.trim() {
                "True" => true,
                "False" => false,
                _ => {
                    println!(
                        "⚠️ Failed to determine whether python at {:?} is running inside a virtualenv",
                        &python
                    );
                    false
                }
            }
        } else {
            false
        }
    } else {
        false
    };

    // If there was success or an error that was not missing cffi, return here
    if !install_cffi {
        return handle_cffi_call_result(python, tempdir, &ffi_py, &output);
    }

    println!("⚠️ cffi not found. Trying to install it");
    // Call pip through python to don't do the wrong thing when python and pip
    // are coming from different environments
    let output = call_python(python, &["-m", "pip", "install", "cffi"])?;
    if !output.status.success() {
        bail!(
            "Installing cffi with `{:?} -m pip install cffi` failed: {}\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\nPlease install cffi yourself.",
            &python,
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }
    println!("🎁 Installed cffi");

    // Try again
    let output = call_python(python, &["-c", &cffi_invocation])?;
    handle_cffi_call_result(python, tempdir, &ffi_py, &output)
}

/// Extracted into a function because this is needed twice
fn handle_cffi_call_result(
    python: &Path,
    tempdir: TempDir,
    ffi_py: &Path,
    output: &Output,
) -> Result<String> {
    if !output.status.success() {
        bail!(
            "Failed to generate cffi declarations using {}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}",
            python.display(),
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?,
        );
    } else {
        // Don't swallow warnings
        io::stderr().write_all(&output.stderr)?;

        let ffi_py_content = fs::read_to_string(&ffi_py)?;
        tempdir.close()?;
        Ok(ffi_py_content)
    }
}

/// Copies the shared library into the module, which is the only extra file needed with bindings
#[allow(clippy::too_many_arguments)]
pub fn write_bindings_module(
    writer: &mut impl ModuleWriter,
    project_layout: &ProjectLayout,
    module_name: &str,
    artifact: &Path,
    python_interpreter: Option<&PythonInterpreter>,
    target: &Target,
    editable: bool,
) -> Result<()> {
    let ext_name = &project_layout.extension_name;
    let so_filename = match python_interpreter {
        Some(python_interpreter) => python_interpreter.get_library_name(ext_name),
        // abi3
        None => {
            if target.is_unix() {
                format!("{base}.abi3.so", base = ext_name)
            } else {
                // Apparently there is no tag for abi3 on windows
                format!("{base}.pyd", base = ext_name)
            }
        }
    };

    if let Some(python_module) = &project_layout.python_module {
        if !editable {
            write_python_part(writer, python_module)
                .context("Failed to add the python module to the package")?;
        }

        if editable {
            let target = project_layout.rust_module.join(&so_filename);
            // Remove existing so file to avoid triggering SIGSEV in running process
            // See https://github.com/PyO3/maturin/issues/758
            let _ = fs::remove_file(&target);
            fs::copy(&artifact, &target).context(format!(
                "Failed to copy {} to {}",
                artifact.display(),
                target.display()
            ))?;
        }

        if !editable {
            let relative = project_layout
                .rust_module
                .strip_prefix(python_module.parent().unwrap())?;
            writer.add_file_with_permissions(relative.join(&so_filename), &artifact, 0o755)?;
        }
    } else {
        let module = PathBuf::from(module_name);
        writer.add_directory(&module)?;
        // Reexport the shared library as if it were the top level module
        writer.add_bytes(
            &module.join("__init__.py"),
            format!(
                r#"from .{module_name} import *

__doc__ = {module_name}.__doc__
if hasattr({module_name}, "__all__"):
    __all__ = {module_name}.__all__"#,
                module_name = module_name
            )
            .as_bytes(),
        )?;
        let type_stub = project_layout
            .rust_module
            .join(format!("{}.pyi", module_name));
        if type_stub.exists() {
            writer.add_bytes(
                &module.join("__init__.pyi"),
                &fs_err::read(type_stub).context("Failed to read type stub file")?,
            )?;
            writer.add_bytes(&module.join("py.typed"), b"")?;
            println!("📖 Found type stub file at {}.pyi", module_name);
        }
        writer.add_file_with_permissions(&module.join(so_filename), &artifact, 0o755)?;
    }

    Ok(())
}

/// Creates the cffi module with the shared library, the cffi declarations and the cffi loader
#[allow(clippy::too_many_arguments)]
pub fn write_cffi_module(
    writer: &mut impl ModuleWriter,
    project_layout: &ProjectLayout,
    crate_dir: &Path,
    target_dir: &Path,
    module_name: &str,
    artifact: &Path,
    python: &Path,
    editable: bool,
) -> Result<()> {
    let cffi_declarations = generate_cffi_declarations(crate_dir, target_dir, python)?;

    let module;

    if let Some(python_module) = &project_layout.python_module {
        if !editable {
            write_python_part(writer, python_module)
                .context("Failed to add the python module to the package")?;
        }

        if editable {
            let base_path = python_module.join(&module_name);
            fs::create_dir_all(&base_path)?;
            let target = base_path.join("native.so");
            fs::copy(&artifact, &target).context(format!(
                "Failed to copy {} to {}",
                artifact.display(),
                target.display()
            ))?;
            File::create(base_path.join("__init__.py"))?.write_all(cffi_init_file().as_bytes())?;
            File::create(base_path.join("ffi.py"))?.write_all(cffi_declarations.as_bytes())?;
        }

        let relative = project_layout
            .rust_module
            .strip_prefix(python_module.parent().unwrap())?;
        module = relative.join(&project_layout.extension_name);
        if !editable {
            writer.add_directory(&module)?;
        }
    } else {
        module = PathBuf::from(module_name);
        writer.add_directory(&module)?;
        let type_stub = project_layout
            .rust_module
            .join(format!("{}.pyi", module_name));
        if type_stub.exists() {
            writer.add_bytes(
                &module.join("__init__.pyi"),
                &fs_err::read(type_stub).context("Failed to read type stub file")?,
            )?;
            writer.add_bytes(&module.join("py.typed"), b"")?;
            println!("📖 Found type stub file at {}.pyi", module_name);
        }
    };

    if !editable || project_layout.python_module.is_none() {
        writer.add_bytes(&module.join("__init__.py"), cffi_init_file().as_bytes())?;
        writer.add_bytes(&module.join("ffi.py"), cffi_declarations.as_bytes())?;
        writer.add_file_with_permissions(&module.join("native.so"), &artifact, 0o755)?;
    }

    Ok(())
}

/// Adds a data directory with a scripts directory with the binary inside it
pub fn write_bin(
    writer: &mut impl ModuleWriter,
    artifact: &Path,
    metadata: &Metadata21,
    bin_name: &str,
) -> Result<()> {
    let data_dir = PathBuf::from(format!(
        "{}-{}.data",
        &metadata.get_distribution_escaped(),
        &metadata.version
    ))
    .join("scripts");

    writer.add_directory(&data_dir)?;

    // We can't use add_file since we need to mark the file as executable
    writer.add_file_with_permissions(&data_dir.join(bin_name), artifact, 0o755)?;
    Ok(())
}

/// Adds a wrapper script that start the wasm binary through wasmtime.
///
/// Note that the wasm binary needs to be written separately by [write_bin]
pub fn write_wasm_launcher(
    writer: &mut impl ModuleWriter,
    metadata: &Metadata21,
    bin_name: &str,
) -> Result<()> {
    let entrypoint_script = format!(
        r#"from pathlib import Path

from wasmtime import Store, Module, Engine, WasiConfig, Linker

import sysconfig

def main():
    # The actual executable
    program_location = Path(sysconfig.get_path("scripts")).joinpath("{}")
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
    "#,
        bin_name
    );

    // We can't use add_file since we want to mark the file as executable
    let launcher_path = Path::new(&metadata.get_distribution_escaped())
        .join(bin_name.replace('-', "_"))
        .with_extension("py");
    writer.add_bytes_with_permissions(&launcher_path, entrypoint_script.as_bytes(), 0o755)?;
    Ok(())
}

/// Adds the python part of a mixed project to the writer,
pub fn write_python_part(
    writer: &mut impl ModuleWriter,
    python_module: impl AsRef<Path>,
) -> Result<()> {
    for absolute in WalkBuilder::new(&python_module).hidden(false).build() {
        let absolute = absolute?.into_path();
        let relative = absolute.strip_prefix(python_module.as_ref().parent().unwrap())?;
        if absolute.is_dir() {
            writer.add_directory(relative)?;
        } else {
            // Ignore native libraries from develop, if any
            if let Some(extension) = relative.extension() {
                if extension.to_string_lossy() == "so" {
                    continue;
                }
            }
            writer
                .add_file(relative, &absolute)
                .context(format!("File to add file from {}", absolute.display()))?;
        }
    }

    Ok(())
}

/// Creates the .dist-info directory and fills it with all metadata files except RECORD
pub fn write_dist_info(
    writer: &mut impl ModuleWriter,
    metadata21: &Metadata21,
    tags: &[String],
) -> Result<()> {
    let dist_info_dir = metadata21.get_dist_info_dir();

    writer.add_directory(&dist_info_dir)?;

    writer.add_bytes(
        &dist_info_dir.join("METADATA"),
        metadata21.to_file_contents()?.as_bytes(),
    )?;

    writer.add_bytes(&dist_info_dir.join("WHEEL"), wheel_file(tags)?.as_bytes())?;

    let mut entry_points = String::new();
    if !metadata21.scripts.is_empty() {
        entry_points.push_str(&entry_points_txt("console_scripts", &metadata21.scripts));
    }
    if !metadata21.gui_scripts.is_empty() {
        entry_points.push_str(&entry_points_txt("gui_scripts", &metadata21.gui_scripts));
    }
    for (entry_type, scripts) in &metadata21.entry_points {
        entry_points.push_str(&entry_points_txt(entry_type, scripts));
    }
    if !entry_points.is_empty() {
        writer.add_bytes(
            &dist_info_dir.join("entry_points.txt"),
            entry_points.as_bytes(),
        )?;
    }

    if !metadata21.license_files.is_empty() {
        let license_files_dir = dist_info_dir.join("license_files");
        writer.add_directory(&license_files_dir)?;
        for path in &metadata21.license_files {
            let filename = path.file_name().with_context(|| {
                format!("missing file name for license file {}", path.display())
            })?;
            writer.add_file(license_files_dir.join(filename), path)?;
        }
    }

    Ok(())
}

/// If any, copies the data files from the data directory, resolving symlinks to their source.
/// We resolve symlinks since we require this rather rigid structure while people might need
/// to save or generate the data in other places
///
/// See https://peps.python.org/pep-0427/#file-contents
pub fn add_data(writer: &mut impl ModuleWriter, data: Option<&Path>) -> Result<()> {
    let possible_data_dir_names = ["data", "scripts", "headers", "purelib", "platlib"];
    if let Some(data) = data {
        for subdir in fs::read_dir(data).context("Failed to read data dir")? {
            let subdir = subdir?;
            let dir_name = subdir
                .file_name()
                .to_str()
                .context("Invalid data dir name")?
                .to_string();
            if !subdir.path().is_dir() || !possible_data_dir_names.contains(&dir_name.as_str()) {
                bail!(
                    "Invalid data dir entry {}. Possible are directories named {}",
                    subdir.path().display(),
                    possible_data_dir_names.join(", ")
                );
            }
            debug!("Adding data from {}", subdir.path().display());
            (|| {
                for file in WalkBuilder::new(subdir.path())
                    .standard_filters(false)
                    .build()
                {
                    let file = file?;
                    let relative = file.path().strip_prefix(data.parent().unwrap())?;

                    if file.path_is_symlink() {
                        // Copy the actual file contents, not the link, so that you can create a
                        // data directory by joining different data sources
                        let source = fs::read_link(file.path())?;
                        writer.add_file(relative, source.parent().unwrap())?;
                    } else if file.path().is_file() {
                        writer.add_file(relative, file.path())?;
                    } else if file.path().is_dir() {
                        writer.add_directory(relative)?;
                    } else {
                        bail!("Can't handle data dir entry {}", file.path().display());
                    }
                }
                Ok(())
            })()
            .with_context(|| format!("Failed to include data from {}", data.display()))?
        }
    }
    Ok(())
}
