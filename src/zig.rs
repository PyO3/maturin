//! Using `zig cc` as c compiler and linker to target a specific glibc/musl version
//! as alternative to the manylinux docker container and for easier cross compiling

use crate::target::Arch;
use crate::{BuildContext, PlatformTag};
use anyhow::{bail, Context, Result};
use fs_err as fs;
use std::env;
#[cfg(target_family = "unix")]
use std::fs::OpenOptions;
use std::io::Write;
#[cfg(target_family = "unix")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::str;

/// Zig linker wrapper
#[derive(Debug, clap::Parser)]
#[clap(name = "zig", setting = clap::AppSettings::Hidden)]
pub enum Zig {
    /// `zig cc` wrapper
    #[clap(name = "cc", setting = clap::AppSettings::TrailingVarArg)]
    Cc {
        /// `zig cc` arguments
        #[clap(takes_value = true, multiple_values = true)]
        args: Vec<String>,
    },
    /// `zig c++` wrapper
    #[clap(name = "c++", setting = clap::AppSettings::TrailingVarArg)]
    Cxx {
        /// `zig c++` arguments
        #[clap(takes_value = true, multiple_values = true)]
        args: Vec<String>,
    },
}

impl Zig {
    /// Execute the underlying zig command
    pub fn execute(&self) -> Result<()> {
        let (cmd, cmd_args) = match self {
            Zig::Cc { args } => ("cc", args),
            Zig::Cxx { args } => ("c++", args),
        };
        // Replace libgcc_s with libunwind
        let cmd_args: Vec<String> = cmd_args
            .iter()
            .map(|arg| {
                let arg = if arg == "-lgcc_s" {
                    "-lunwind".to_string()
                } else if arg.starts_with('@') && arg.ends_with("linker-arguments") {
                    // rustc passes arguments to linker via an @-file when arguments are too long
                    // See https://github.com/rust-lang/rust/issues/41190
                    let content = fs::read(arg.trim_start_matches('@'))?;
                    let link_args = str::from_utf8(&content)?.replace("-lgcc_s", "-lunwind");
                    fs::write(arg.trim_start_matches('@'), link_args.as_bytes())?;
                    arg.to_string()
                } else {
                    arg.to_string()
                };
                Ok(arg)
            })
            .collect::<Result<_>>()?;
        let (zig, zig_args) = Self::find_zig()?;
        let mut child = Command::new(zig)
            .args(zig_args)
            .arg(cmd)
            .args(cmd_args)
            .spawn()
            .with_context(|| format!("Failed to run `zig {}`", cmd))?;
        let status = child.wait().expect("Failed to wait on zig child process");
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }

    /// Search for `python -m ziglang` first and for `zig` second.
    /// That way we use the zig from `maturin[ziglang]` first,
    /// but users or distributions can also insert their own zig
    pub fn find_zig() -> Result<(String, Vec<String>)> {
        Self::find_zig_python()
            .or_else(|_| Self::find_zig_bin())
            .context("Failed to find zig")
    }

    /// Detect the plain zig binary
    fn find_zig_bin() -> Result<(String, Vec<String>)> {
        let output = Command::new("zig").arg("version").output()?;
        let version_str =
            str::from_utf8(&output.stdout).context("`zig version` didn't return utf8 output")?;
        Self::validate_zig_version(version_str)?;
        Ok(("zig".to_string(), Vec::new()))
    }

    /// Detect the Python ziglang package
    fn find_zig_python() -> Result<(String, Vec<String>)> {
        let output = Command::new("python3")
            .args(&["-m", "ziglang", "version"])
            .output()?;
        let version_str = str::from_utf8(&output.stdout)
            .context("`python3 -m ziglang version` didn't return utf8 output")?;
        Self::validate_zig_version(version_str)?;
        Ok((
            "python3".to_string(),
            vec!["-m".to_string(), "ziglang".to_string()],
        ))
    }

    fn validate_zig_version(version: &str) -> Result<()> {
        let min_ver = semver::Version::new(0, 9, 0);
        let version = semver::Version::parse(version.trim())?;
        if version >= min_ver {
            Ok(())
        } else {
            bail!(
                "zig version {} is too old, need at least {}",
                version,
                min_ver
            )
        }
    }
}

/// We want to use `zig cc` as linker and c compiler. We want to call `python -m ziglang cc`, but
/// cargo only accepts a path to an executable as linker, so we add a wrapper script. We then also
/// use the wrapper script to pass arguments and substitute an unsupported argument.
///
/// We create different files for different args because otherwise cargo might skip recompiling even
/// if the linker target changed
pub fn prepare_zig_linker(context: &BuildContext) -> Result<(PathBuf, PathBuf)> {
    let target = &context.target;
    let arch = if target.cross_compiling() {
        if matches!(target.target_arch(), Arch::Armv7L) {
            "armv7".to_string()
        } else {
            target.target_arch().to_string()
        }
    } else {
        "native".to_string()
    };
    let file_ext = if cfg!(windows) { "bat" } else { "sh" };
    let (zig_cc, zig_cxx, cc_args) = match context.platform_tag {
        // Not sure branch even has any use case, but it doesn't hurt to support it
        None | Some(PlatformTag::Linux) => (
            format!("zigcc-gnu.{}", file_ext),
            format!("zigcxx-gnu.{}", file_ext),
            format!("-target {}-linux-gnu", arch),
        ),
        Some(PlatformTag::Musllinux { x, y }) => {
            println!("⚠️  Warning: zig with musl is unstable");
            (
                format!("zigcc-musl-{}-{}.{}", x, y, file_ext),
                format!("zigcxx-musl-{}-{}.{}", x, y, file_ext),
                format!("-target {}-linux-musl", arch),
            )
        }
        Some(PlatformTag::Manylinux { x, y }) => (
            format!("zigcc-gnu-{}-{}.{}", x, y, file_ext),
            format!("zigcxx-gnu-{}-{}.{}", x, y, file_ext),
            // https://github.com/ziglang/zig/issues/10050#issuecomment-956204098
            format!("-target {}-linux-gnu.{}.{}", arch, x, y),
        ),
    };

    let zig_linker_dir = dirs::cache_dir()
        // If the really is no cache dir, cwd will also do
        .unwrap_or_else(|| env::current_dir().expect("Failed to get current dir"))
        .join(env!("CARGO_PKG_NAME"))
        .join(env!("CARGO_PKG_VERSION"));
    fs::create_dir_all(&zig_linker_dir)?;

    let zig_cc = zig_linker_dir.join(zig_cc);
    let zig_cxx = zig_linker_dir.join(zig_cxx);
    write_linker_wrapper(&zig_cc, "cc", &cc_args)?;
    write_linker_wrapper(&zig_cxx, "c++", &cc_args)?;

    Ok((zig_cc, zig_cxx))
}

#[cfg(target_family = "unix")]
fn write_linker_wrapper(path: &Path, command: &str, args: &str) -> Result<()> {
    let mut custom_linker_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o700)
        .open(path)?;
    let current_exe = if let Ok(maturin) = env::var("CARGO_BIN_EXE_maturin") {
        PathBuf::from(maturin)
    } else {
        env::current_exe()?
    };
    writeln!(&mut custom_linker_file, "#!/bin/bash")?;
    writeln!(
        &mut custom_linker_file,
        "{} zig {} -- {} $@",
        current_exe.display(),
        command,
        args
    )?;
    Ok(())
}

/// Write a zig cc wrapper batch script for windows
#[cfg(not(target_family = "unix"))]
fn write_linker_wrapper(path: &Path, command: &str, args: &str) -> Result<()> {
    let mut custom_linker_file = fs::File::create(path)?;
    let current_exe = if let Ok(maturin) = env::var("CARGO_BIN_EXE_maturin") {
        PathBuf::from(maturin)
    } else {
        env::current_exe()?
    };
    writeln!(
        &mut custom_linker_file,
        "{} zig {} -- {} %*",
        current_exe.display(),
        command,
        args
    )?;
    Ok(())
}
