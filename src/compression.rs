use clap::{CommandFactory, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Debug, Copy, Clone, PartialEq, Eq, ValueEnum)]
/// Wheel ZIP compression method. May only be compatible with recent package manager versions.
// See https://docs.rs/zip/latest/zip/write/struct.FileOptions.html#method.compression_level
pub enum CompressionMethod {
    #[default]
    /// Deflate compression (levels 0-9, default 6)
    // NOTE: The levels will change if we enable Zopfli!
    Deflated,
    /// No compression
    Stored,
    /// BZIP2 compression (levels 0-9, default 6)
    Bzip2,
    /// Zstandard compression (supported from Python 3.14; levels -7-22, default 3)
    Zstd,
}
impl CompressionMethod {
    /// Default level for this compression method
    // NOTE: This should match the default from the `zip` crates.
    pub fn default_level(self) -> i64 {
        match self {
            CompressionMethod::Deflated => 6,
            CompressionMethod::Stored => 0,
            CompressionMethod::Bzip2 => 6,
            CompressionMethod::Zstd => 3,
        }
    }
    /// Allowed levels for the method.
    pub fn level_range(self) -> std::ops::RangeInclusive<i64> {
        match self {
            CompressionMethod::Deflated => 0..=9,
            CompressionMethod::Stored => 0..=0,
            CompressionMethod::Bzip2 => 0..=9,
            CompressionMethod::Zstd => -7..=22,
        }
    }
}
impl From<CompressionMethod> for zip::CompressionMethod {
    fn from(source: CompressionMethod) -> Self {
        match source {
            CompressionMethod::Deflated => zip::CompressionMethod::Deflated,
            CompressionMethod::Stored => zip::CompressionMethod::Stored,
            CompressionMethod::Bzip2 => zip::CompressionMethod::Bzip2,
            CompressionMethod::Zstd => zip::CompressionMethod::ZSTD,
        }
    }
}
#[derive(Debug, Default, Serialize, Deserialize, clap::Parser, Clone, Copy, Eq, PartialEq)]
/// Wheel ZIP compression options.
pub struct CompressionOptions {
    /// Zip compression method. Only Stored and Deflated are currently compatible with all
    /// package managers.
    #[arg(long, value_enum, default_value_t = CompressionMethod::default())]
    pub compression_method: CompressionMethod,

    /// Zip compression level. Defaults to method default.
    #[arg(long, allow_negative_numbers = true)]
    pub compression_level: Option<i64>,
}
impl CompressionOptions {
    /// Validate arguments, exit on error
    pub fn validate(&self) {
        if let Some(level) = self.compression_level {
            let range = self.compression_method.level_range();
            if !range.contains(&level) {
                let mut cmd = Self::command();
                cmd.error(
                    clap::error::ErrorKind::ArgumentConflict,
                    format!(
                        "Invalid level {} for compression method {:?}. Use a level in range {:?}",
                        level, self.compression_method, range
                    ),
                )
                .exit();
            }
        }
    }
    /// No compression
    pub fn none() -> Self {
        let method = CompressionMethod::Stored;
        Self {
            compression_method: method,
            compression_level: Some(method.default_level()),
        }
    }

    pub(crate) fn get_file_options(&self) -> zip::write::FileOptions<'static, ()> {
        let method = if cfg!(feature = "faster-tests") {
            // Unlike users which can use the develop subcommand, the tests have to go through
            // packing a zip which pip than has to unpack. This makes this 2-3 times faster
            CompressionMethod::Stored
        } else {
            self.compression_method
        };

        let mut options =
            zip::write::SimpleFileOptions::default().compression_method(method.into());
        // `zip` also has default compression levels, which should match our own, but we pass them
        // explicitly to ensure consistency. The exception is the `Stored` method, which must have
        // a `compression_level` of `None`.
        options = options.compression_level(if method == CompressionMethod::Stored {
            None
        } else {
            Some(self.compression_level.unwrap_or(method.default_level()))
        });
        options
    }
}
