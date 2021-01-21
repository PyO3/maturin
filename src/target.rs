use anyhow::{bail, format_err, Result};
use platform_info::*;
use platforms::Platform;
use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

/// All supported operating system
#[derive(Debug, Clone, Eq, PartialEq)]
enum OS {
    Linux,
    Windows,
    Macos,
    FreeBSD,
}

/// Decides how to handle manylinux compliance
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub enum Manylinux {
    /// Use the manylinux2010 tag
    Manylinux2010,
    /// Use the manylinux2014 tag
    Manylinux2014,
    /// Use the native linux tag
    Off,
}

impl fmt::Display for Manylinux {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Manylinux::Manylinux2010 => write!(f, "manylinux2010"),
            Manylinux::Manylinux2014 => write!(f, "manylinux2014"),
            Manylinux::Off => write!(f, "linux"),
        }
    }
}

impl FromStr for Manylinux {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "2010" => Ok(Manylinux::Manylinux2010),
            "2014" => Ok(Manylinux::Manylinux2014),
            "off" => Ok(Manylinux::Off),
            _ => Err("Invalid value for the manylinux option"),
        }
    }
}

/// All supported CPU architectures
#[derive(Debug, Clone, Eq, PartialEq)]
enum Arch {
    AARCH64,
    ARMV7L,
    POWERPC64LE,
    POWERPC64,
    X86,
    X86_64,
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Arch::AARCH64 => write!(f, "aarch64"),
            Arch::ARMV7L => write!(f, "armv7l"),
            Arch::POWERPC64LE => write!(f, "ppc64le"),
            Arch::POWERPC64 => write!(f, "ppc64"),
            Arch::X86 => write!(f, "i686"),
            Arch::X86_64 => write!(f, "x86_64"),
        }
    }
}

/// The part of the current platform that is relevant when building wheels and is supported
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Target {
    os: OS,
    arch: Arch,
}

impl Target {
    /// Uses the given target triple or tries the guess the current target by using the one used
    /// for compilation
    ///
    /// Fails if the target triple isn't supported
    pub fn from_target_triple(target_triple: Option<String>) -> Result<Self> {
        let platform = if let Some(ref target_triple) = target_triple {
            Platform::find(target_triple)
                .ok_or_else(|| format_err!("Unknown target triple {}", target_triple))?
        } else {
            Platform::guess_current()
                .ok_or_else(|| format_err!("Could guess the current platform"))?
        };

        let os = match platform.target_os {
            platforms::target::OS::Linux => OS::Linux,
            platforms::target::OS::Windows => OS::Windows,
            platforms::target::OS::MacOS => OS::Macos,
            platforms::target::OS::FreeBSD => OS::FreeBSD,
            unsupported => bail!("The operating system {:?} is not supported", unsupported),
        };

        let arch = match platform.target_arch {
            platforms::target::Arch::X86_64 => Arch::X86_64,
            platforms::target::Arch::X86 => Arch::X86,
            platforms::target::Arch::ARM => Arch::ARMV7L,
            platforms::target::Arch::AARCH64 => Arch::AARCH64,
            platforms::target::Arch::POWERPC64
                if platform.target_triple.starts_with("powerpc64-") =>
            {
                Arch::POWERPC64
            }
            platforms::target::Arch::POWERPC64
                if platform.target_triple.starts_with("powerpc64le-") =>
            {
                Arch::POWERPC64LE
            }
            unsupported => bail!("The architecture {:?} is not supported", unsupported),
        };

        // bail on any unsupported targets
        match (&os, &arch) {
            (OS::FreeBSD, Arch::AARCH64) => bail!("aarch64 is not supported for FreeBSD"),
            (OS::FreeBSD, Arch::ARMV7L) => bail!("armv7l is not supported for FreeBSD"),
            (OS::FreeBSD, Arch::X86) => bail!("32-bit wheels are not supported for FreeBSD"),
            (OS::FreeBSD, Arch::X86_64) => {
                match PlatformInfo::new() {
                    Ok(_) => {}
                    Err(error) => bail!(error),
                };
            }
            (OS::Macos, Arch::ARMV7L) => bail!("armv7l is not supported for macOS"),
            (OS::Macos, Arch::X86) => bail!("32-bit wheels are not supported for macOS"),
            (OS::Windows, Arch::AARCH64) => bail!("aarch64 is not supported for Windows"),
            (OS::Windows, Arch::ARMV7L) => bail!("armv7l is not supported for Windows"),
            (_, _) => {}
        }
        Ok(Target { os, arch })
    }

    /// Returns whether the platform is 64 bit or 32 bit
    pub fn pointer_width(&self) -> usize {
        match self.arch {
            Arch::AARCH64 => 64,
            Arch::ARMV7L => 32,
            Arch::POWERPC64 => 64,
            Arch::POWERPC64LE => 64,
            Arch::X86 => 32,
            Arch::X86_64 => 64,
        }
    }

    /// Returns true if the current platform is linux or mac os
    pub fn is_unix(&self) -> bool {
        self.os != OS::Windows
    }

    /// Returns true if the current platform is linux
    pub fn is_linux(&self) -> bool {
        self.os == OS::Linux
    }

    /// Returns true if the current platform is freebsd
    pub fn is_freebsd(&self) -> bool {
        self.os == OS::FreeBSD
    }

    /// Returns true if the current platform is mac os
    pub fn is_macos(&self) -> bool {
        self.os == OS::Macos
    }

    /// Returns true if the current platform is windows
    pub fn is_windows(&self) -> bool {
        self.os == OS::Windows
    }

    /// Returns the platform part of the tag for the wheel name for cffi wheels
    pub fn get_platform_tag(&self, manylinux: &Manylinux, universal2: bool) -> String {
        match (&self.os, &self.arch) {
            (OS::FreeBSD, Arch::X86_64) => {
                let info = match PlatformInfo::new() {
                    Ok(info) => info,
                    Err(error) => panic!(error),
                };
                let release = info.release().replace(".", "_").replace("-", "_");
                format!("freebsd_{}_amd64", release)
            }
            (OS::Linux, _) => format!("{}_{}", manylinux, self.arch),
            (OS::Macos, Arch::X86_64) => {
                if universal2 {
                    "macosx_10_9_universal2".to_string()
                } else {
                    "macosx_10_7_x86_64".to_string()
                }
            }
            (OS::Macos, Arch::AARCH64) => {
                if universal2 {
                    "macosx_10_9_universal2".to_string()
                } else {
                    "macosx_11_0_arm64".to_string()
                }
            }
            (OS::Windows, Arch::X86) => "win32".to_string(),
            (OS::Windows, Arch::X86_64) => "win_amd64".to_string(),
            (_, _) => panic!("unsupported target should not have reached get_platform_tag()"),
        }
    }

    /// Returns the tags for the WHEEL file for cffi wheels
    pub fn get_py3_tags(&self, manylinux: &Manylinux, universal2: bool) -> Vec<String> {
        vec![format!(
            "py3-none-{}",
            self.get_platform_tag(&manylinux, universal2)
        )]
    }

    /// Returns the platform for the tag in the shared libraries file name
    pub fn get_shared_platform_tag(&self) -> &'static str {
        match (&self.os, &self.arch) {
            (OS::FreeBSD, _) => "", // according imp.get_suffixes(), there are no such
            (OS::Linux, Arch::AARCH64) => "aarch64-linux-gnu", // aka armv8-linux-gnueabihf
            (OS::Linux, Arch::ARMV7L) => "arm-linux-gnueabihf",
            (OS::Linux, Arch::POWERPC64) => "powerpc64-linux-gnu",
            (OS::Linux, Arch::POWERPC64LE) => "powerpc64le-linux-gnu",
            (OS::Linux, Arch::X86) => "i386-linux-gnu", // not i686
            (OS::Linux, Arch::X86_64) => "x86_64-linux-gnu",
            (OS::Macos, Arch::X86_64) => "darwin",
            (OS::Macos, Arch::AARCH64) => "darwin",
            (OS::Windows, Arch::X86) => "win32",
            (OS::Windows, Arch::X86_64) => "win_amd64",
            (OS::Macos, _) => {
                panic!("unsupported macOS Arch should not have reached get_shared_platform_tag()")
            }
            (OS::Windows, _) => {
                panic!("unsupported Windows Arch should not have reached get_shared_platform_tag()")
            }
        }
    }

    /// Returns the path to the python executable inside a venv
    pub fn get_venv_python(&self, venv_base: impl AsRef<Path>) -> PathBuf {
        if self.is_windows() {
            let path = venv_base.as_ref().join("Scripts").join("python.exe");
            if path.exists() {
                path
            } else {
                // for conda environment
                venv_base.as_ref().join("python.exe")
            }
        } else {
            venv_base.as_ref().join("bin").join("python")
        }
    }

    /// Returns the directory where the binaries are stored inside a venv
    pub fn get_venv_bin_dir(&self, venv_base: impl AsRef<Path>) -> PathBuf {
        if self.is_windows() {
            venv_base.as_ref().join("Scripts")
        } else {
            venv_base.as_ref().join("bin")
        }
    }

    /// Returns the path to the python executable
    ///
    /// For windows it's always python.exe for unix it's first the venv's `python`
    /// and then `python3`
    pub fn get_python(&self) -> PathBuf {
        if self.is_windows() {
            PathBuf::from("python.exe")
        } else if env::var_os("VIRTUAL_ENV").is_some() {
            PathBuf::from("python")
        } else {
            PathBuf::from("python3")
        }
    }

    /// Returns the tags for the platform without python version
    pub fn get_universal_tags(
        &self,
        manylinux: &Manylinux,
        universal2: bool,
    ) -> (String, Vec<String>) {
        let tag = format!(
            "py3-none-{platform}",
            platform = self.get_platform_tag(&manylinux, universal2)
        );
        let tags = self.get_py3_tags(&manylinux, universal2);
        (tag, tags)
    }
}
