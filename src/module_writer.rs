//! The wheel format is (mostly) specified in PEP 427

use crate::build_context::ProjectLayout;
use crate::Metadata21;
use crate::PythonInterpreter;
use crate::Target;
use base64;
use failure::{bail, Context, Error, ResultExt};
use flate2::write::GzEncoder;
use flate2::Compression;
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
use std::str;
use tempfile::tempdir;
use walkdir::WalkDir;
use zip::{self, ZipWriter};

/// Allows writing the module to a wheel or add it directly to the virtualenv
pub trait ModuleWriter {
    /// Adds a directory relative to the module base path
    fn add_directory(&mut self, path: impl AsRef<Path>) -> Result<(), Error>;

    /// Adds a file with bytes as content in target relative to the module base path
    fn add_bytes(&mut self, target: impl AsRef<Path>, bytes: &[u8]) -> Result<(), Error> {
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
    ) -> Result<(), Error>;

    /// Copies the source file the the target path relative to the module base path
    fn add_file(
        &mut self,
        target: impl AsRef<Path>,
        source: impl AsRef<Path>,
    ) -> Result<(), Error> {
        let mut file = File::open(&source).context("Failed to read file at {}")?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        self.add_bytes(target, &buffer)?;
        Ok(())
    }
}

/// A [ModuleWriter] that adds the module somewhere in the filesystem, e.g. in a virtualenv
pub struct PathWriter {
    base_path: PathBuf,
}

impl PathWriter {
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

        Ok(PathWriter { base_path })
    }

    /// Writes the module to the given path
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        Self {
            base_path: path.as_ref().to_path_buf(),
        }
    }

    /// Removes a directory relative to the base path if it exists.
    ///
    /// This is to clean up the contents of an older develop call
    pub fn delete_dir(&self, relative: impl AsRef<Path>) -> Result<(), Error> {
        let absolute = self.base_path.join(relative);
        if absolute.exists() {
            fs::remove_dir_all(&absolute)
                .context(format!("Failed to remove {}", absolute.display()))?;
        }

        Ok(())
    }
}

impl ModuleWriter for PathWriter {
    fn add_directory(&mut self, path: impl AsRef<Path>) -> Result<(), Error> {
        fs::create_dir_all(self.base_path.join(path))?;
        Ok(())
    }

    fn add_bytes_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        bytes: &[u8],
        _permissions: u32,
    ) -> Result<(), Error> {
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

        file.write_all(bytes)?;
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
    fn add_directory(&mut self, _path: impl AsRef<Path>) -> Result<(), Error> {
        Ok(()) // We don't need to create directories in zip archives
    }

    fn add_bytes_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        bytes: &[u8],
        permissions: u32,
    ) -> Result<(), Error> {
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
    ) -> Result<WheelWriter, Error> {
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

        write_dist_info(&mut builder, &metadata21, &scripts, &tags)?;

        Ok(builder)
    }

    /// Creates the record file and finishes the zip
    pub fn finish(mut self) -> Result<PathBuf, io::Error> {
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        self.zip
            .start_file(self.record_file.to_str().unwrap(), options)?;
        for (filename, hash, len) in self.record {
            self.zip
                .write_all(format!("{},sha256={},{}\n", filename, hash, len).as_bytes())?;
        }
        self.zip
            .write_all(format!("{},,\n", self.record_file.to_str().unwrap()).as_bytes())?;

        self.zip.finish()?;
        Ok(self.wheel_path)
    }
}

/// Creates a .tar.gz archive containing the source distribution
pub struct SDistWriter {
    tar: tar::Builder<GzEncoder<File>>,
    path: PathBuf,
}

impl ModuleWriter for SDistWriter {
    fn add_directory(&mut self, _path: impl AsRef<Path>) -> Result<(), Error> {
        Ok(())
    }

    fn add_bytes_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        bytes: &[u8],
        permissions: u32,
    ) -> Result<(), Error> {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(permissions);
        header.set_cksum();
        self.tar
            .append_data(&mut header, &target, bytes)
            .context(format!(
                "Failed to add {} bytes to sdist as {}",
                bytes.len(),
                target.as_ref().display()
            ))?;
        Ok(())
    }

    fn add_file(
        &mut self,
        target: impl AsRef<Path>,
        source: impl AsRef<Path>,
    ) -> Result<(), Error> {
        self.tar
            .append_path_with_name(&source, &target)
            .context(format!(
                "Failed to add file from {} to sdist as {}",
                source.as_ref().display(),
                target.as_ref().display(),
            ))?;
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

        Ok(Self { tar, path })
    }

    /// Finished the .tar.gz archive
    pub fn finish(mut self) -> Result<PathBuf, io::Error> {
        self.tar.finish()?;
        Ok(self.path)
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
fn entry_points_txt(entrypoints: &HashMap<String, String, impl std::hash::BuildHasher>) -> String {
    entrypoints
        .iter()
        .fold("[console_scripts]\n".to_owned(), |text, (k, v)| {
            text + k + "=" + v + "\n"
        })
}

/// Glue code that exposes `lib`.
fn cffi_init_file() -> &'static str {
    r#"__all__ = ["lib", "ffi"]

import os
from .ffi import ffi

lib = ffi.dlopen(os.path.join(os.path.dirname(__file__), 'native.so'), 4098)
del os
"#
}

/// Returns the content of what will become ffi.py by invocing cbindgen and cffi
///
/// First we check if user has provided their own header at `target/header.h`, otherwise
/// we run cbindgen to generate onw.
///
/// We're using the cffi recompiler, which reads the header, translates them into instructions
/// how to load the shared library without the header and then writes those instructions to a
/// file called `ffi.py`. This `ffi.py` will expose an object called `ffi`. This object is used
/// in `__init__.py` to load the shared library into a module called `lib`.
pub fn generate_cffi_declarations(crate_dir: &Path, python: &PathBuf) -> Result<String, Error> {
    let tempdir = tempdir()?;
    let maybe_header = crate_dir.join("target").join("header.h");

    let header;
    if maybe_header.is_file() {
        println!(
            "💼 Using the existing header at {}",
            maybe_header.display()
        );
        header = maybe_header;
    } else {
        let bindings = cbindgen::Builder::new()
            .with_no_includes()
            .with_language(cbindgen::Language::C)
            .with_crate(crate_dir)
            .generate()
            .context("Failed to run cbindgen")?;
        header = tempdir.as_ref().join("header.h");
        bindings.write_to_file(&header);
    }

    let ffi_py = tempdir.as_ref().join("ffi.py");

    // Using raw strings is important because on windows there are path like
    // `C:\Users\JohnDoe\AppData\Local\TEmpl\pip-wheel-asdf1234` where the \U
    // would otherwise be a broken unicode exscape sequence
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

    let output = Command::new(python)
        .args(&["-c", &cffi_invocation])
        .output()?;
    if !output.status.success() {
        bail!(
            "Failed to generate cffi declarations using {}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}",
            python.display(),
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?,
        );
    }

    // Don't swallow warnings
    std::io::stderr().write_all(&output.stderr)?;

    let ffi_py_content = fs::read_to_string(ffi_py)?;
    tempdir.close()?;
    Ok(ffi_py_content)
}

/// Copies the shared library into the module, which is the only extra file needed with bindings
pub fn write_bindings_module(
    writer: &mut impl ModuleWriter,
    project_layout: &ProjectLayout,
    module_name: &str,
    artifact: &Path,
    python_interpreter: &PythonInterpreter,
    develop: bool,
) -> Result<(), Error> {
    let so_filename = python_interpreter.get_library_name(&module_name);

    match project_layout {
        ProjectLayout::Mixed(ref python_module) => {
            write_python_part(writer, python_module, &module_name)
                .context("Failed to add the python module to the package")?;

            if develop {
                fs::copy(&artifact, python_module.join(&so_filename))?;
            }

            writer.add_file(Path::new(&module_name).join(&so_filename), &artifact)?;
        }
        ProjectLayout::PureRust => {
            writer.add_file(so_filename, &artifact)?;
        }
    }

    Ok(())
}

/// Creates the cffi module with the shared library, the cffi declarations and the cffi loader
pub fn write_cffi_module(
    writer: &mut impl ModuleWriter,
    project_layout: &ProjectLayout,
    crate_dir: &Path,
    module_name: &str,
    artifact: &Path,
    python: &PathBuf,
    develop: bool,
) -> Result<(), Error> {
    let cffi_declarations = generate_cffi_declarations(&crate_dir, python)?;

    let module;

    match project_layout {
        ProjectLayout::Mixed(ref python_module) => {
            write_python_part(writer, python_module, &module_name)
                .context("Failed to add the python module to the package")?;

            if develop {
                let base_path = python_module.join(&module_name);
                fs::create_dir_all(&base_path)?;
                fs::copy(&artifact, base_path.join("native.so"))?;
                File::create(base_path.join("__init__.py"))?
                    .write_all(cffi_init_file().as_bytes())?;
                File::create(base_path.join("ffi.py"))?.write_all(cffi_declarations.as_bytes())?;
            }

            module = PathBuf::from(module_name).join(module_name);
        }
        ProjectLayout::PureRust => module = PathBuf::from(module_name),
    };

    writer.add_directory(&module)?;
    writer.add_bytes(&module.join("__init__.py"), cffi_init_file().as_bytes())?;
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

/// Adds the python part of a mixed project to the writer,
/// excluding older versions of the native library or generated cffi declarations
pub fn write_python_part(
    writer: &mut impl ModuleWriter,
    python_module: impl AsRef<Path>,
    module_name: impl AsRef<Path>,
) -> Result<(), Error> {
    for absolute in WalkDir::new(&python_module) {
        let absolute = absolute?.into_path();

        let relaitve = absolute.strip_prefix(python_module.as_ref().parent().unwrap())?;

        // Ignore the cffi folder from develop, if any
        if relaitve.starts_with(module_name.as_ref().join(&module_name)) {
            continue;
        }

        if absolute.is_dir() {
            writer.add_directory(relaitve)?;
        } else {
            // Ignore native libraries from develop, if any
            if let Some(extension) = relaitve.extension() {
                if extension.to_string_lossy() == "so" {
                    continue;
                }
            }
            writer
                .add_file(relaitve, &absolute)
                .context(format!("File to add file from {}", absolute.display()))?;
        }
    }

    Ok(())
}

/// Creates the .dist-info directory and fills it with all metadata files except RECORD
pub fn write_dist_info(
    writer: &mut impl ModuleWriter,
    metadata21: &Metadata21,
    scripts: &HashMap<String, String, impl std::hash::BuildHasher>,
    tags: &[String],
) -> Result<(), Error> {
    let dist_info_dir = metadata21.get_dist_info_dir();

    writer.add_directory(&dist_info_dir)?;

    writer.add_bytes(
        &dist_info_dir.join("METADATA"),
        metadata21.to_file_contents().as_bytes(),
    )?;

    writer.add_bytes(&dist_info_dir.join("WHEEL"), wheel_file(tags).as_bytes())?;

    if !scripts.is_empty() {
        writer.add_bytes(
            &dist_info_dir.join("entry_points.txt"),
            entry_points_txt(scripts).as_bytes(),
        )?;
    }

    Ok(())
}
