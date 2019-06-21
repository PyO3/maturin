use failure::{bail, format_err, Error};
use platforms;
use platforms::target::Arch;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use target_info;

/// All supported operating system
#[derive(Debug, Clone, Eq, PartialEq)]
enum OS {
    Linux,
    Windows,
    Macos,
}

/// Decides how to handle manylinux compliance
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub enum Manylinux {
    /// Use the manylinux1 tag and check for compliance
    Manylinux1,
    /// Use the manylinux1 tag but don't check for compliance
    Manylinux1Unchecked,
    /// Use manylinux2010 tag and check for compliance
    Manylinux2010,
    /// Use the manylinux2010 tag but don't check for compliance
    Manylinux2010Unchecked,
    /// Use the native linux tag
    Off,
}

impl FromStr for Manylinux {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "1" => Ok(Manylinux::Manylinux1),
            "1-unchecked" => Ok(Manylinux::Manylinux1Unchecked),
            "2010" => Ok(Manylinux::Manylinux2010),
            "2010-unchecked" => Ok(Manylinux::Manylinux2010Unchecked),
            "off" => Ok(Manylinux::Off),
            _ => Err("Invalid value for the manylinux option"),
        }
    }
}

/// The part of the current platform that is relevant when building wheels and is supported
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Target {
    os: OS,
    is_64_bit: bool,
}

impl Target {
    /// Returns the target pyo3-pack was compiled for
    pub fn current() -> Self {
        let os = match target_info::Target::os() {
            "linux" => OS::Linux,
            "windows" => OS::Windows,
            "macos" => OS::Macos,
            unsupported => panic!("The platform {} is not supported", unsupported),
        };

        let is_64_bit = match target_info::Target::pointer_width() {
            "64" => true,
            "32" => false,
            unsupported => panic!("The pointer width {} is not supported ಠ_ಠ", unsupported),
        };

        Target { os, is_64_bit }
    }

    /// Uses the given target triple or tries the guess the current target by using the one used
    /// for compilation
    ///
    /// Fails if the target triple isn't supported
    pub fn from_target_triple(target_triple: Option<String>) -> Result<Self, Error> {
        let platform = if let Some(ref target_triple) = target_triple {
            platforms::find(target_triple)
                .ok_or_else(|| format_err!("Unknown target triple {}", target_triple))?
        } else {
            platforms::guess_current()
                .ok_or_else(|| format_err!("Could guess the current platform"))?
        };

        let os = match platform.target_os {
            platforms::target::OS::Linux => OS::Linux,
            platforms::target::OS::Windows => OS::Windows,
            platforms::target::OS::MacOS => OS::Macos,
            unsupported => bail!("The operating system {:?} is not supported", unsupported),
        };

        let is_64_bit = match platform.target_arch {
            Arch::X86_64 => true,
            Arch::X86 => false,
            unsupported => bail!("The architecture {:?} is not supported", unsupported),
        };

        Ok(Target { os, is_64_bit })
    }

    /// Returns whether the platform is 64 bit or 32 bit
    pub fn pointer_width(&self) -> usize {
        if self.is_64_bit {
            64
        } else {
            32
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

    /// Returns true if the current platform is mac os
    pub fn is_macos(&self) -> bool {
        self.os == OS::Macos
    }

    /// Returns true if the current platform is windows
    pub fn is_windows(&self) -> bool {
        self.os == OS::Windows
    }

    /// Returns the platform part of the tag for the wheel name for cffi wheels
    pub fn get_platform_tag(&self, manylinux: &Manylinux) -> &'static str {
        match (&self.os, self.is_64_bit, manylinux) {
            (&OS::Linux, true, Manylinux::Off) => "linux_x86_64",
            (&OS::Linux, false, Manylinux::Off) => "linux_i686",
            (&OS::Linux, true, Manylinux::Manylinux1) => "manylinux1_x86_64",
            (&OS::Linux, true, Manylinux::Manylinux1Unchecked) => "manylinux1_x86_64",
            (&OS::Linux, true, Manylinux::Manylinux2010) => "manylinux2010_x86_64",
            (&OS::Linux, true, Manylinux::Manylinux2010Unchecked) => "manylinux2010_x86_64",
            (&OS::Linux, false, Manylinux::Manylinux1) => "manylinux1_i686",
            (&OS::Linux, false, Manylinux::Manylinux1Unchecked) => "manylinux1_i686",
            (&OS::Linux, false, Manylinux::Manylinux2010) => "manylinux2010_i686",
            (&OS::Linux, false, Manylinux::Manylinux2010Unchecked) => "manylinux2010_i686",
            (&OS::Windows, true, _) => "win_amd64",
            (&OS::Windows, false, _) => "win32",
            (&OS::Macos, true, _) => "macosx_10_7_x86_64",
            (&OS::Macos, false, _) => panic!("32-bit wheels are not supported for mac os"),
        }
    }

    /// Returns the tags for the WHEEL file for cffi wheels
    pub fn get_py2_and_py3_tags(&self, manylinux: &Manylinux) -> Vec<String> {
        vec![
            format!("py2-none-{}", self.get_platform_tag(&manylinux)),
            format!("py3-none-{}", self.get_platform_tag(&manylinux)),
        ]
    }

    /// Returns the platform for the tag in the shared libaries file name
    pub fn get_shared_platform_tag(&self) -> &'static str {
        match self.os {
            OS::Linux => {
                if self.is_64_bit {
                    "x86_64-linux-gnu"
                } else {
                    "x86-linux-gnu"
                }
            }
            OS::Macos => "darwin",
            OS::Windows => {
                if self.is_64_bit {
                    "win_amd64"
                } else {
                    "win32"
                }
            }
        }
    }

    /// Returns the path to the python executable inside a venv
    pub fn get_venv_python(&self, venv_base: impl AsRef<Path>) -> PathBuf {
        if self.is_windows() {
            venv_base.as_ref().join("Scripts").join("python.exe")
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
    pub fn get_universal_tags(&self, manylinux: &Manylinux) -> (String, Vec<String>) {
        let tag = format!(
            "py2.py3-none-{platform}",
            platform = self.get_platform_tag(&manylinux)
        );
        let tags = self.get_py2_and_py3_tags(&manylinux);
        (tag, tags)
    }
}
