//! The wheel format is (mostly) specified in PEP 427

use base64;
use failure::{Context, Error};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io;
use std::io::{Read, Write};
#[cfg(not(target_os = "windows"))]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::str;
use tempfile::tempdir;
use zip::{self, ZipWriter};
use Metadata21;
use PythonInterpreter;
use Target;

/// Allows writing the module to a wheel or add it directly to the virtualenv
pub trait ModuleWriter {
    /// Adds a directory relative to the module base path
    fn add_directory(&mut self, path: impl AsRef<Path>) -> Result<(), io::Error>;

    /// Adds a file with bytes as content in target relative to the module base path
    fn add_bytes(&mut self, target: impl AsRef<Path>, bytes: &[u8]) -> Result<(), io::Error> {
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
    ) -> Result<(), io::Error>;

    /// Copies the source file the the target path relative to the module base path
    fn add_file(
        &mut self,
        target: impl AsRef<Path>,
        source: impl AsRef<Path>,
    ) -> Result<(), io::Error> {
        let mut file = File::open(source)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        self.add_bytes(target, &buffer)
    }
}

/// A [ModuleWriter] that adds the module somewhere in the filesystem, e.g. in a virtualenv
pub struct DevelopModuleWriter {
    base_path: PathBuf,
}

impl DevelopModuleWriter {
    /// Creates a [ModuleWriter] that adds the modul to the current virtualenv
    pub fn venv(target: &Target, venv_dir: &Path) -> Result<Self, Error> {
        let interpreter =
            PythonInterpreter::check_executable(target.get_venv_python(&venv_dir), &target)?
                .ok_or_else(|| {
                    Context::new(
                        "Expected `python` to be a python interpreter inside a virtualenv ಠ_ಠ",
                    )
                })?;

        let python_dir = format!("python{}.{}", interpreter.major, interpreter.minor);

        let base_path = if target.is_unix() {
            venv_dir.join("lib").join(python_dir).join("site-packages")
        } else {
            venv_dir.join("Lib").join("site-packages")
        };

        Ok(DevelopModuleWriter { base_path })
    }
}

impl ModuleWriter for DevelopModuleWriter {
    fn add_directory(&mut self, path: impl AsRef<Path>) -> Result<(), io::Error> {
        fs::create_dir_all(self.base_path.join(path))
    }

    fn add_bytes_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        bytes: &[u8],
        _permissions: u32,
    ) -> Result<(), io::Error> {
        let path = self.base_path.join(target);

        // We only need to set the executable bit on unix
        let mut file = {
            #[cfg(not(target_os = "windows"))]
            {
                fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .mode(_permissions)
                    .open(path)?
            }
            #[cfg(target_os = "windows")]
            {
                File::create(path)?
            }
        };

        file.write_all(bytes)
    }
}

/// A glorified zip builder, mostly useful for writing the record file of a wheel
pub struct WheelWriter {
    zip: ZipWriter<File>,
    record: Vec<(String, String, usize)>,
    dist_info_dir: PathBuf,
    wheel_path: PathBuf,
}

impl ModuleWriter for WheelWriter {
    fn add_directory(&mut self, _path: impl AsRef<Path>) -> Result<(), io::Error> {
        Ok(()) // We don't need to create directories in zip archives
    }

    fn add_bytes_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        bytes: &[u8],
        permissions: u32,
    ) -> Result<(), io::Error> {
        // So apparently we must use unix style paths for pypi's checks to succeed; Without
        // the replacing we get a "400 Client Error: Invalid distribution file."
        let target = target.as_ref().to_str().unwrap().replace("\\", "/");
        let options = zip::write::FileOptions::default()
            .unix_permissions(permissions)
            .compression_method(zip::CompressionMethod::Deflated);
        self.zip.start_file(target.clone(), options)?;
        self.zip.write_all(&bytes)?;

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
        scripts: &HashMap<String, String>,
        tags: &[String],
    ) -> Result<WheelWriter, io::Error> {
        let wheel_path = wheel_dir.join(format!(
            "{}-{}-{}.whl",
            metadata21.get_distribution_escaped(),
            metadata21.get_version_escaped(),
            tag
        ));

        let file = File::create(&wheel_path)?;

        let dist_info_dir = PathBuf::from(format!(
            "{}-{}.dist-info",
            &metadata21.get_distribution_escaped(),
            &metadata21.get_version_escaped()
        ));

        let mut builder = WheelWriter {
            zip: ZipWriter::new(file),
            record: Vec::new(),
            dist_info_dir: dist_info_dir.clone(),
            wheel_path,
        };

        builder.add_bytes(
            &dist_info_dir.join("METADATA"),
            metadata21.to_file_contents().as_bytes(),
        )?;

        builder.add_bytes(&dist_info_dir.join("WHEEL"), wheel_file(tags).as_bytes())?;

        if !scripts.is_empty() {
            builder.add_bytes(
                &dist_info_dir.join("entry_points.txt"),
                entry_points_txt(scripts).as_bytes(),
            )?;
        }

        Ok(builder)
    }

    /// Creates the record file and finishes the zip
    pub fn finish(mut self) -> Result<PathBuf, io::Error> {
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let record_file = self.dist_info_dir.join("RECORD");
        self.zip
            .start_file(record_file.to_str().unwrap(), options)?;
        for (filename, hash, len) in self.record {
            self.zip
                .write_all(format!("{},sha256={},{}\n", filename, hash, len).as_bytes())?;
        }
        self.zip
            .write_all(format!("{},,\n", record_file.to_str().unwrap()).as_bytes())?;

        self.zip.finish()?;
        Ok(self.wheel_path)
    }
}

fn wheel_file(tags: &[String]) -> String {
    let mut wheel_file = format!(
        "Wheel-Version: 1.0
Generator: {name} ({version})
Root-Is-Purelib: false
",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
    );

    for tag in tags {
        wheel_file += &format!("Tag: {}\n", tag);
    }

    wheel_file
}

/// https://packaging.python.org/specifications/entry-points/
fn entry_points_txt(entrypoints: &HashMap<String, String>) -> String {
    entrypoints
        .iter()
        .fold("[console_scripts]\n".to_owned(), |text, (k, v)| {
            text + k + "=" + v + "\n"
        })
}

fn cffi_init_file() -> &'static str {
    r#"__all__ = ["lib", "ffi"]

import os
from .ffi import ffi

lib = ffi.dlopen(os.path.join(os.path.dirname(__file__), 'native.so'), 4098)
del os
"#
}

/// Returns the content of what will become ffi.py by invocing cffi
///
/// We're using the cffi recompiler, which reads the header, translates them into instructions
/// how to load the shared library without the header and then writes those instructions to a
/// file called `ffi.py`. This `ffi.py` will expose an object called `ffi`. This object is used
/// in `__init__.py` to load the shared library into a module called `lib`.
pub fn generate_cffi_declarations(header: &Path, python: &PathBuf) -> Result<String, Error> {
    let is_include = Regex::new("#include <.*>").unwrap();

    // We need to remove the includes from the header because cffi can't process them and there's
    // no option to deactivate them in cbindgen
    let filtered_header = fs::read_to_string(header)?
        .lines()
        .filter(|line| !is_include.is_match(line))
        .collect::<Vec<&str>>()
        .join("");

    let tempdir = tempdir()?;
    let ffi_py = tempdir.as_ref().join("ffi.py");
    let header_h = tempdir.as_ref().join("header.h");

    File::create(&header_h)?.write_all(filtered_header.as_bytes())?;

    let cffi_invocation = format!(
        r#"
import cffi
from cffi import recompiler

ffi = cffi.FFI()
with open("{header_h}") as header:
    ffi.cdef(header.read())
ffi.set_source("a", None)
recompiler.make_py_source(ffi, "ffi", "{ffi_py}")
"#,
        ffi_py = ffi_py.display(),
        header_h = header_h.display(),
    );

    let output = Command::new(python)
        .args(&["-c", &cffi_invocation])
        .stderr(Stdio::inherit())
        .output()?;
    if !output.status.success() {
        bail!("Failed to generate cffi declarations");
    }

    Ok(fs::read_to_string(ffi_py)?)
}

/// Copies the shared library into the module, which is the only extra file needed with bindings
pub fn write_bindings_module(
    writer: &mut impl ModuleWriter,
    module_name: &str,
    artifact: &Path,
    python_interpreter: &PythonInterpreter,
) -> Result<(), Error> {
    let so_filename = PathBuf::from(format!(
        "{}{}",
        module_name,
        python_interpreter.get_library_extension()
    ));

    writer.add_file(&so_filename, &artifact)?;

    Ok(())
}

/// Creates the cffi module with the shared library, the cffi declarations and the cffi loader
pub fn write_cffi_module(
    writer: &mut impl ModuleWriter,
    module_name: &str,
    artifact: &Path,
    python: &PathBuf,
) -> Result<(), Error> {
    let module = Path::new(module_name);

    // This should do until cbindgen gets their serde issues fixed
    let header = artifact
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("header.h");

    writer.add_directory(&module)?;
    writer.add_bytes(&module.join("__init__.py"), cffi_init_file().as_bytes())?;
    let cffi_declarations = generate_cffi_declarations(&header, python)?;
    writer.add_bytes(&module.join("ffi.py"), cffi_declarations.as_bytes())?;
    writer.add_file(&module.join("native.so"), &artifact)?;

    Ok(())
}

/// Adds a data directory with a scripts directory with the binary inside it
pub fn write_bin(
    writer: &mut impl ModuleWriter,
    artifact: &Path,
    metadata: &Metadata21,
    bin_name: &OsStr,
) -> Result<(), Error> {
    let data_dir = PathBuf::from(format!(
        "{}-{}.data",
        &metadata.get_distribution_escaped(),
        &metadata.version
    ))
    .join("scripts");

    writer.add_directory(&data_dir)?;

    // We can't use add_file since we need to mark the file as executable
    let mut file = File::open(artifact)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    writer.add_bytes_with_permissions(&data_dir.join(bin_name), &buffer, 0o755)?;
    Ok(())
}
