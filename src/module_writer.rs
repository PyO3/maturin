//! The wheel format is (mostly) specified in PEP 427
use crate::project_layout::ProjectLayout;
use crate::target::Os;
use crate::{
    pyproject_toml::Format, BridgeModel, Metadata21, PyProjectToml, PythonInterpreter, Target,
};
use anyhow::{anyhow, bail, Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use fs_err as fs;
use fs_err::File;
use ignore::overrides::Override;
use ignore::WalkBuilder;
use normpath::PathExt as _;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::env;
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
use zip::{self, DateTime, ZipWriter};

/// Allows writing the module to a wheel or add it directly to the virtualenv
pub trait ModuleWriter {
    /// Adds a directory relative to the module base path
    fn add_directory(&mut self, path: impl AsRef<Path>) -> Result<()>;

    /// Adds a file with bytes as content in target relative to the module base path
    fn add_bytes(&mut self, target: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
        debug!("Adding {}", target.as_ref().display());
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
        self.add_file_with_permissions(target, source, 0o644)
    }

    /// Copies the source file the the target path relative to the module base path while setting
    /// the given unix permissions
    fn add_file_with_permissions(
        &mut self,
        target: impl AsRef<Path>,
        source: impl AsRef<Path>,
        permissions: u32,
    ) -> Result<()> {
        let target = target.as_ref();
        let source = source.as_ref();
        debug!("Adding {} from {}", target.display(), source.display());

        let read_failed_context = format!("Failed to read {}", source.display());
        let mut file = File::open(source).context(read_failed_context.clone())?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).context(read_failed_context)?;
        self.add_bytes_with_permissions(target, &buffer, permissions)
            .context(format!("Failed to write to {}", target.display()))?;
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
            PythonInterpreter::check_executable(target.get_venv_python(venv_dir), target, bridge)?
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
                .write_all(format!("{filename},sha256={hash},{len}\n").as_bytes())
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
        let target = self.base_path.join(path);
        debug!("Adding directory {}", target.display());
        fs::create_dir_all(target)?;
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

        let hash = base64::encode_config(Sha256::digest(bytes), base64::URL_SAFE_NO_PAD);
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
    excludes: Option<Override>,
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
        let target = target.as_ref();
        if self.exclude(target) {
            return Ok(());
        }
        // The zip standard mandates using unix style paths
        let target = target.to_str().unwrap().replace('\\', "/");

        // Unlike users which can use the develop subcommand, the tests have to go through
        // packing a zip which pip than has to unpack. This makes this 2-3 times faster
        let compression_method = if cfg!(feature = "faster-tests") {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };

        let mut options = zip::write::FileOptions::default()
            .unix_permissions(permissions)
            .compression_method(compression_method);
        let mtime = self.mtime().ok();
        if let Some(mtime) = mtime {
            options = options.last_modified_time(mtime);
        }

        self.zip.start_file(target.clone(), options)?;
        self.zip.write_all(bytes)?;

        let hash = base64::encode_config(Sha256::digest(bytes), base64::URL_SAFE_NO_PAD);
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
        excludes: Option<Override>,
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
            excludes,
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
        if project_layout.python_module.is_some() || !project_layout.python_packages.is_empty() {
            let absolute_path = project_layout
                .python_dir
                .normalize()
                .with_context(|| {
                    format!(
                        "failed to normalize path `{}`",
                        project_layout.python_dir.display()
                    )
                })?
                .into_path_buf();
            if let Some(python_path) = absolute_path.to_str() {
                let name = metadata21.get_distribution_escaped();
                let target = format!("{name}.pth");
                debug!("Adding {} from {}", target, python_path);
                self.add_bytes(target, python_path.as_bytes())?;
            } else {
                eprintln!("⚠️ source code path contains non-Unicode sequences, editable installs may not work.");
            }
        }
        Ok(())
    }

    /// Returns `true` if the given path should be excluded
    fn exclude(&self, path: impl AsRef<Path>) -> bool {
        if let Some(excludes) = &self.excludes {
            excludes.matched(path.as_ref(), false).is_whitelist()
        } else {
            false
        }
    }

    /// Returns a DateTime representing the value SOURCE_DATE_EPOCH environment variable
    /// Note that the earliest timestamp a zip file can represent is 1980-01-01
    fn mtime(&self) -> Result<DateTime> {
        let epoch: i64 = env::var("SOURCE_DATE_EPOCH")?.parse()?;
        let dt = time::OffsetDateTime::from_unix_timestamp(epoch)?;
        let min_dt = time::Date::from_calendar_date(1980, time::Month::January, 1)
            .unwrap()
            .midnight()
            .assume_offset(time::UtcOffset::UTC);
        let dt = dt.max(min_dt);

        let dt = DateTime::try_from(dt).map_err(|_| anyhow!("Failed to build zip DateTime"))?;
        Ok(dt)
    }

    /// Creates the record file and finishes the zip
    pub fn finish(mut self) -> Result<PathBuf, io::Error> {
        let compression_method = if cfg!(feature = "faster-tests") {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };

        let mut options = zip::write::FileOptions::default().compression_method(compression_method);
        let mtime = self.mtime().ok();
        if let Some(mtime) = mtime {
            options = options.last_modified_time(mtime);
        }

        let record_filename = self.record_file.to_str().unwrap().replace('\\', "/");
        debug!("Adding {}", record_filename);
        self.zip.start_file(&record_filename, options)?;
        for (filename, hash, len) in self.record {
            self.zip
                .write_all(format!("{filename},sha256={hash},{len}\n").as_bytes())?;
        }
        // Write the record for the RECORD file itself
        self.zip
            .write_all(format!("{record_filename},,\n").as_bytes())?;

        self.zip.finish()?;
        Ok(self.wheel_path)
    }
}

/// Creates a .tar.gz archive containing the source distribution
pub struct SDistWriter {
    tar: tar::Builder<GzEncoder<File>>,
    path: PathBuf,
    files: HashSet<PathBuf>,
    excludes: Option<Override>,
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
        if self.exclude(target) {
            return Ok(());
        }

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
        if self.exclude(source) {
            return Ok(());
        }
        let target = target.as_ref();
        if source == self.path {
            eprintln!(
                "⚠️  Warning: Attempting to include the sdist output tarball {} into itself! Check 'cargo package --list' output.",
                source.display()
            );
            return Ok(());
        }
        if self.files.contains(target) {
            // Ignore duplicate files
            return Ok(());
        }
        debug!("Adding {} from {}", target.display(), source.display());

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
    pub fn new(
        wheel_dir: impl AsRef<Path>,
        metadata21: &Metadata21,
        excludes: Option<Override>,
    ) -> Result<Self, io::Error> {
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
            excludes,
        })
    }

    /// Returns `true` if the given path should be excluded
    fn exclude(&self, path: impl AsRef<Path>) -> bool {
        if let Some(excludes) = &self.excludes {
            excludes.matched(path.as_ref(), false).is_whitelist()
        } else {
            false
        }
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
        writeln!(wheel_file, "Tag: {tag}")?;
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
        .fold(format!("[{entry_type}]\n"), |text, (k, v)| {
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
    Command::new(python)
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

        let mut config = cbindgen::Config::from_root_or_default(crate_dir);
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
        debug!("Generated header.h at {}", header.display());
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

    let output = call_python(python, ["-c", &cffi_invocation])?;
    let install_cffi = if !output.status.success() {
        // First, check whether the error was cffi not being installed
        let last_line = str::from_utf8(&output.stderr)?.lines().last().unwrap_or("");
        if last_line == "ModuleNotFoundError: No module named 'cffi'" {
            // Then check whether we're running in a virtualenv.
            // We don't want to modify any global environment
            // https://stackoverflow.com/a/42580137/3549270
            let output = call_python(
                python,
                ["-c", "import sys\nprint(sys.base_prefix != sys.prefix)"],
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
    let output = call_python(
        python,
        [
            "-m",
            "pip",
            "install",
            "--disable-pip-version-check",
            "cffi",
        ],
    )?;
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
    let output = call_python(python, ["-c", &cffi_invocation])?;
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

        let ffi_py_content = fs::read_to_string(ffi_py)?;
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
    pyproject_toml: Option<&PyProjectToml>,
) -> Result<()> {
    let ext_name = &project_layout.extension_name;
    let so_filename = match python_interpreter {
        Some(python_interpreter) => python_interpreter.get_library_name(ext_name),
        // abi3
        None => {
            if target.is_unix() {
                format!("{ext_name}.abi3.so")
            } else {
                // Apparently there is no tag for abi3 on windows
                format!("{ext_name}.pyd")
            }
        }
    };

    if !editable {
        write_python_part(writer, project_layout, pyproject_toml)
            .context("Failed to add the python module to the package")?;
    }
    if let Some(python_module) = &project_layout.python_module {
        if editable {
            let target = project_layout.rust_module.join(&so_filename);
            // Remove existing so file to avoid triggering SIGSEV in running process
            // See https://github.com/PyO3/maturin/issues/758
            debug!("Removing {}", target.display());
            let _ = fs::remove_file(&target);

            debug!("Copying {} to {}", artifact.display(), target.display());
            fs::copy(artifact, &target).context(format!(
                "Failed to copy {} to {}",
                artifact.display(),
                target.display()
            ))?;
        } else {
            let relative = project_layout
                .rust_module
                .strip_prefix(python_module.parent().unwrap())
                .unwrap();
            writer.add_file_with_permissions(relative.join(&so_filename), artifact, 0o755)?;
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
    __all__ = {module_name}.__all__"#
            )
            .as_bytes(),
        )?;
        let type_stub = project_layout
            .rust_module
            .join(format!("{module_name}.pyi"));
        if type_stub.exists() {
            println!("📖 Found type stub file at {module_name}.pyi");
            writer.add_file(&module.join("__init__.pyi"), type_stub)?;
            writer.add_bytes(&module.join("py.typed"), b"")?;
        }
        writer.add_file_with_permissions(&module.join(so_filename), artifact, 0o755)?;
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
    pyproject_toml: Option<&PyProjectToml>,
) -> Result<()> {
    let cffi_declarations = generate_cffi_declarations(crate_dir, target_dir, python)?;

    if !editable {
        write_python_part(writer, project_layout, pyproject_toml)
            .context("Failed to add the python module to the package")?;
    }

    let module;
    if let Some(python_module) = &project_layout.python_module {
        if editable {
            let base_path = python_module.join(&project_layout.extension_name);
            fs::create_dir_all(&base_path)?;
            let target = base_path.join("native.so");
            fs::copy(artifact, &target).context(format!(
                "Failed to copy {} to {}",
                artifact.display(),
                target.display()
            ))?;
            File::create(base_path.join("__init__.py"))?.write_all(cffi_init_file().as_bytes())?;
            File::create(base_path.join("ffi.py"))?.write_all(cffi_declarations.as_bytes())?;
        }

        let relative = project_layout
            .rust_module
            .strip_prefix(python_module.parent().unwrap())
            .unwrap();
        module = relative.join(&project_layout.extension_name);
        if !editable {
            writer.add_directory(&module)?;
        }
    } else {
        module = PathBuf::from(module_name);
        writer.add_directory(&module)?;
        let type_stub = project_layout
            .rust_module
            .join(format!("{module_name}.pyi"));
        if type_stub.exists() {
            println!("📖 Found type stub file at {module_name}.pyi");
            writer.add_file(&module.join("__init__.pyi"), type_stub)?;
            writer.add_bytes(&module.join("py.typed"), b"")?;
        }
    };

    if !editable || project_layout.python_module.is_none() {
        writer.add_bytes(&module.join("__init__.py"), cffi_init_file().as_bytes())?;
        writer.add_bytes(&module.join("ffi.py"), cffi_declarations.as_bytes())?;
        writer.add_file_with_permissions(&module.join("native.so"), artifact, 0o755)?;
    }

    Ok(())
}

/// uniffi.toml
#[derive(Debug, serde::Deserialize)]
struct UniFfiToml {
    #[serde(default)]
    bindings: HashMap<String, UniFfiBindingsConfig>,
}

/// `bindings` section of uniffi.toml
#[derive(Debug, serde::Deserialize)]
struct UniFfiBindingsConfig {
    cdylib_name: Option<String>,
}

#[derive(Debug, Clone)]
struct UniFfiBindings {
    name: String,
    cdylib: String,
    path: PathBuf,
}

fn generate_uniffi_bindings(
    crate_dir: &Path,
    target_dir: &Path,
    target_os: Os,
) -> Result<UniFfiBindings> {
    let binding_dir = target_dir.join("maturin").join("uniffi");
    fs::create_dir_all(&binding_dir)?;

    let pattern = crate_dir.join("src").join("*.udl");
    let udls = glob::glob(pattern.to_str().unwrap())?
        .map(|p| p.unwrap())
        .collect::<Vec<_>>();
    if udls.is_empty() {
        bail!("No UDL files found in {}", crate_dir.join("src").display());
    } else if udls.len() > 1 {
        bail!(
            "Multiple UDL files found in {}",
            crate_dir.join("src").display()
        );
    }

    let mut cmd = Command::new("uniffi-bindgen");
    cmd.args([
        "generate",
        "--no-format",
        "--language",
        "python",
        "--out-dir",
    ]);
    cmd.arg(&binding_dir);

    let udl = &udls[0];
    let config_file = crate_dir.join("uniffi.toml");
    let mut cdylib_name = None;
    if config_file.is_file() {
        let uniffi_toml: UniFfiToml = toml::from_str(&fs::read_to_string(&config_file)?)?;
        cdylib_name = uniffi_toml
            .bindings
            .get("python")
            .and_then(|py| py.cdylib_name.clone());
        cmd.arg("--config");
        cmd.arg(config_file);
    }
    cmd.arg(udl);
    debug!("Running {:?}", cmd);
    let mut child = cmd.spawn().context(
        "Failed to run uniffi-bindgen, did you install it? Try `cargo install uniffi_bindgen`",
    )?;
    let exit_status = child.wait().context("Failed to run uniffi-bindgen")?;
    if !exit_status.success() {
        bail!("Command {:?} failed", cmd);
    }

    let py_binding_name = udl.file_stem().unwrap();
    let py_binding = binding_dir.join(py_binding_name).with_extension("py");
    let name = py_binding_name.to_str().unwrap().to_string();

    // uniffi bindings hardcoded the extension filenames
    let cdylib_name = match cdylib_name {
        Some(name) => name,
        None => format!("uniffi_{name}"),
    };
    let cdylib = match target_os {
        Os::Macos => format!("lib{cdylib_name}.dylib"),
        Os::Windows => format!("{cdylib_name}.dll"),
        _ => format!("lib{cdylib_name}.so"),
    };

    Ok(UniFfiBindings {
        name,
        cdylib,
        path: py_binding,
    })
}

/// Creates the uniffi module with the shared library
#[allow(clippy::too_many_arguments)]
pub fn write_uniffi_module(
    writer: &mut impl ModuleWriter,
    project_layout: &ProjectLayout,
    crate_dir: &Path,
    target_dir: &Path,
    module_name: &str,
    artifact: &Path,
    target_os: Os,
    editable: bool,
    pyproject_toml: Option<&PyProjectToml>,
) -> Result<()> {
    let UniFfiBindings {
        name: binding_name,
        cdylib,
        path: uniffi_binding,
    } = generate_uniffi_bindings(crate_dir, target_dir, target_os)?;
    let py_init = format!("from .{binding_name} import *  # NOQA\n");

    if !editable {
        write_python_part(writer, project_layout, pyproject_toml)
            .context("Failed to add the python module to the package")?;
    }

    let module;
    if let Some(python_module) = &project_layout.python_module {
        if editable {
            let base_path = python_module.join(&project_layout.extension_name);
            fs::create_dir_all(&base_path)?;
            let target = base_path.join(&cdylib);
            fs::copy(artifact, &target).context(format!(
                "Failed to copy {} to {}",
                artifact.display(),
                target.display()
            ))?;

            File::create(base_path.join("__init__.py"))?.write_all(py_init.as_bytes())?;
            let target = base_path.join(&binding_name).with_extension("py");
            fs::copy(&uniffi_binding, &target).context(format!(
                "Failed to copy {} to {}",
                uniffi_binding.display(),
                target.display()
            ))?;
        }

        let relative = project_layout
            .rust_module
            .strip_prefix(python_module.parent().unwrap())
            .unwrap();
        module = relative.join(&project_layout.extension_name);
        if !editable {
            writer.add_directory(&module)?;
        }
    } else {
        module = PathBuf::from(module_name);
        writer.add_directory(&module)?;
        let type_stub = project_layout
            .rust_module
            .join(format!("{module_name}.pyi"));
        if type_stub.exists() {
            println!("📖 Found type stub file at {module_name}.pyi");
            writer.add_file(&module.join("__init__.pyi"), type_stub)?;
            writer.add_bytes(&module.join("py.typed"), b"")?;
        }
    };

    if !editable || project_layout.python_module.is_none() {
        writer.add_bytes(&module.join("__init__.py"), py_init.as_bytes())?;
        writer.add_file(
            module.join(binding_name).with_extension("py"),
            uniffi_binding,
        )?;
        writer.add_file_with_permissions(&module.join(cdylib), artifact, 0o755)?;
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
    project_layout: &ProjectLayout,
    pyproject_toml: Option<&PyProjectToml>,
) -> Result<()> {
    let python_dir = &project_layout.python_dir;
    let mut python_packages = Vec::new();
    if let Some(python_module) = project_layout.python_module.as_ref() {
        python_packages.push(python_module.to_path_buf());
    }
    for package in &project_layout.python_packages {
        let package_path = python_dir.join(package);
        if python_packages.iter().any(|p| *p == package_path) {
            continue;
        }
        python_packages.push(package_path);
    }

    for package in python_packages {
        for absolute in WalkBuilder::new(package).hidden(false).build() {
            let absolute = absolute?.into_path();
            let relative = absolute.strip_prefix(python_dir).unwrap();
            if absolute.is_dir() {
                writer.add_directory(relative)?;
            } else {
                // Ignore native libraries from develop, if any
                if let Some(extension) = relative.extension() {
                    if extension.to_string_lossy() == "so" {
                        debug!("Ignoring native library {}", relative.display());
                        continue;
                    }
                }
                writer
                    .add_file(relative, &absolute)
                    .context(format!("File to add file from {}", absolute.display()))?;
            }
        }
    }

    // Include additional files
    if let Some(pyproject) = pyproject_toml {
        // FIXME: in src-layout pyproject.toml isn't located directly in python dir
        let pyproject_dir = python_dir;
        if let Some(glob_patterns) = pyproject.include() {
            for pattern in glob_patterns
                .iter()
                .filter_map(|glob_pattern| glob_pattern.targets(Format::Sdist))
            {
                println!("📦 Including files matching \"{pattern}\"");
                for source in glob::glob(&pyproject_dir.join(pattern).to_string_lossy())
                    .expect("No files found for pattern")
                    .filter_map(Result::ok)
                {
                    let target = source.strip_prefix(pyproject_dir)?.to_path_buf();
                    if source.is_dir() {
                        writer.add_directory(target)?;
                    } else {
                        writer.add_file(target, source)?;
                    }
                }
            }
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
                    let relative = file.path().strip_prefix(data.parent().unwrap()).unwrap();

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

#[cfg(test)]
mod tests {
    use ignore::overrides::OverrideBuilder;

    use super::*;

    #[test]
    // The mechanism is the same for wheel_writer
    fn sdist_writer_excludes() -> Result<(), Box<dyn std::error::Error>> {
        let metadata = Metadata21::default();
        let perm = 0o777;

        // No excludes
        let tmp_dir = TempDir::new()?;
        let mut writer = SDistWriter::new(&tmp_dir, &metadata, None)?;
        assert!(writer.files.is_empty());
        writer.add_bytes_with_permissions("test", &[], perm)?;
        assert_eq!(writer.files.len(), 1);
        writer.finish()?;
        tmp_dir.close()?;

        // A test filter
        let tmp_dir = TempDir::new()?;
        let mut excludes = OverrideBuilder::new(&tmp_dir);
        excludes.add("test*")?;
        excludes.add("!test2")?;
        let mut writer = SDistWriter::new(&tmp_dir, &metadata, Some(excludes.build()?))?;
        writer.add_bytes_with_permissions("test1", &[], perm)?;
        writer.add_bytes_with_permissions("test3", &[], perm)?;
        assert!(writer.files.is_empty());
        writer.add_bytes_with_permissions("test2", &[], perm)?;
        assert!(!writer.files.is_empty());
        writer.add_bytes_with_permissions("yes", &[], perm)?;
        assert_eq!(writer.files.len(), 2);
        writer.finish()?;
        tmp_dir.close()?;

        Ok(())
    }
}
