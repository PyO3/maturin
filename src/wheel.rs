//! The wheel format is (mostly) specified in PEP 427

use base64;
use failure::Error;
use metadata::WheelMetadata;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str;
use target_info::Target;
use zip::{self, ZipWriter};
use PythonInterpreter;

/// A glorified zip builder, mostly useful for writing the record file of a wheel
pub struct WheelBuilder {
    zip: ZipWriter<File>,
    record: Vec<(String, String, usize)>,
}

impl WheelBuilder {
    /// Create a new wheel file which can be subsequently expanded
    pub fn new(target_file: &Path) -> Result<WheelBuilder, io::Error> {
        let file = File::create(target_file)?;

        Ok(WheelBuilder {
            zip: ZipWriter::new(file),
            record: Vec::new(),
        })
    }

    /// Adds a file to wheel with the given bytes as contnt
    pub fn add_bytes(&mut self, target_file: &Path, bytes: &[u8]) -> Result<(), io::Error> {
        let target_str = target_file.to_str().unwrap();
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        self.zip.start_file(target_str, options)?;
        self.zip.write_all(&bytes)?;

        let hash = base64::encode_config(&Sha256::digest(bytes), base64::URL_SAFE_NO_PAD);
        self.record
            .push((target_str.to_string(), hash, bytes.len()));

        Ok(())
    }

    /// Copies a file to the wheel
    pub fn add_file(&mut self, target_file: &Path, src_file: &Path) -> Result<(), io::Error> {
        let mut file = File::open(src_file)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        self.add_bytes(target_file, &buffer)
    }

    /// Creates the record file and finishes the zip
    pub fn finish(mut self, record_file: &Path) -> Result<(), io::Error> {
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        self.zip.start_file(record_file.to_str().unwrap(), options)?;
        for (filename, hash, len) in self.record {
            self.zip
                .write_all(format!("{},sha256={},{}\n", filename, hash, len).as_bytes())?;
        }
        self.zip
            .write_all(format!("{},,\n", record_file.to_str().unwrap()).as_bytes())?;

        self.zip.finish()?;
        Ok(())
    }
}

fn wheel_file(tag: &str) -> String {
    format!(
        "Wheel-Version: 1.0
Generator: {name} ({version})
Root-Is-Purelib: false
Tag: {tag}
",
        name = env!("CARGO_PKG_NAME"),
        tag = tag,
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// https://packaging.python.org/specifications/entry-points/
fn entry_points_txt(entrypoints: &HashMap<String, String>) -> String {
    entrypoints
        .iter()
        .fold("[console_scripts]\n".to_owned(), |text, (k, v)| {
            text + k + "=" + v + "\n"
        })
}

/// Generates the correct filename for the current platform and the given python version
///
/// Note that PEP 3149 is only valid for 3.2 - 3.4 for mac and linux and the 3.5 release notes
/// are wrong. The syntax is adapted from the (also incorrect) release notes of python 3.5:
/// https://docs.python.org/3/whatsnew/3.5.html#build-and-c-api-changes
///
/// Examples for x86 on Python 3.5m:
/// Linux:   steinlaus.cpython-35m-x86_64-linux-gnu.so
/// Windows: steinlaus.cp35-win_amd64.pyd
/// Mac:     steinlaus.cpython-35m-darwin.so
fn get_library_name(basename: &str, python_version: &PythonInterpreter) -> String {
    if python_version.major == 2 {
        return format!("{basename}.so", basename = basename);
    }

    assert!(python_version.major == 3 && python_version.minor >= 5);
    assert_eq!(python_version.has_u, false);

    match Target::os() {
        "linux" => format!(
            "{basename}.cpython-{major}{minor}{abiflags}-{architecture}-{os}.so",
            basename = basename,
            major = python_version.major,
            minor = python_version.minor,
            abiflags = "m",
            architecture = Target::arch(),
            os = format!("{}-{}", Target::os(), Target::env()),
        ),
        "macos" => format!(
            "{basename}.cpython-{major}{minor}{abiflags}-darwin.so",
            basename = basename,
            major = python_version.major,
            minor = python_version.minor,
            abiflags = "m",
        ),
        "windows" => format!(
            "{basename}.cp{major}{minor}-{platform}.pyd",
            basename = basename,
            major = python_version.major,
            minor = python_version.minor,
            platform = if Target::pointer_width() == "64" {
                "win_amd64"
            } else {
                "win32"
            },
        ),
        _ => panic!("This platform is not supported"),
    }
}

/// Creates the complete wheel after the compilation finished
pub fn build_wheel(
    metadata: &WheelMetadata,
    python_version: &PythonInterpreter,
    artifact: &Path,
    wheel_path: &Path,
) -> Result<(), Error> {
    println!("Building the wheel to {}", wheel_path.display());

    let dist_info_dir = PathBuf::from(format!(
        "{}-{}.dist-info",
        &metadata.metadata21.name, &metadata.metadata21.version
    ));

    let so_filename = PathBuf::from(get_library_name(&metadata.metadata21.name, &python_version));

    let mut builder = WheelBuilder::new(&wheel_path)?;
    builder.add_file(&so_filename, &artifact)?;
    builder.add_bytes(
        &dist_info_dir.join("WHEEL"),
        wheel_file(&python_version.get_tag()).as_bytes(),
    )?;
    builder.add_bytes(
        &dist_info_dir.join("METADATA"),
        metadata.metadata21.to_file_contents().as_bytes(),
    )?;
    if !metadata.scripts.is_empty() {
        builder.add_bytes(
            &dist_info_dir.join("entry_points.txt"),
            entry_points_txt(&metadata.scripts).as_bytes(),
        )?;
    }
    builder.finish(&dist_info_dir.join("RECORD"))?;

    Ok(())
}
