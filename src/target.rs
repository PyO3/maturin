use crate::{Manylinux, PythonInterpreter};
use anyhow::{bail, format_err, Result};
use platform_info::*;
use platforms::target::Env;
use platforms::Platform;
use std::env;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;

/// All supported operating system
#[derive(Debug, Clone, Eq, PartialEq)]
enum Os {
    Linux,
    Windows,
    Macos,
    FreeBsd,
}

/// All supported CPU architectures
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Arch {
    Aarch64,
    Armv7L,
    Powerpc64Le,
    Powerpc64,
    X86,
    X86_64,
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Arch::Aarch64 => write!(f, "aarch64"),
            Arch::Armv7L => write!(f, "armv7l"),
            Arch::Powerpc64Le => write!(f, "ppc64le"),
            Arch::Powerpc64 => write!(f, "ppc64"),
            Arch::X86 => write!(f, "i686"),
            Arch::X86_64 => write!(f, "x86_64"),
        }
    }
}

/// The part of the current platform that is relevant when building wheels and is supported
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Target {
    os: Os,
    arch: Arch,
    env: Option<Env>,
    triple: &'static str,
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
            platforms::target::OS::Linux => Os::Linux,
            platforms::target::OS::Windows => Os::Windows,
            platforms::target::OS::MacOS => Os::Macos,
            platforms::target::OS::FreeBSD => Os::FreeBsd,
            unsupported => bail!("The operating system {:?} is not supported", unsupported),
        };

        let arch = match platform.target_arch {
            platforms::target::Arch::X86_64 => Arch::X86_64,
            platforms::target::Arch::X86 => Arch::X86,
            platforms::target::Arch::ARM => Arch::Armv7L,
            platforms::target::Arch::AARCH64 => Arch::Aarch64,
            platforms::target::Arch::POWERPC64
                if platform.target_triple.starts_with("powerpc64-") =>
            {
                Arch::Powerpc64
            }
            platforms::target::Arch::POWERPC64
                if platform.target_triple.starts_with("powerpc64le-") =>
            {
                Arch::Powerpc64Le
            }
            unsupported => bail!("The architecture {:?} is not supported", unsupported),
        };

        // bail on any unsupported targets
        match (&os, &arch) {
            (Os::FreeBsd, Arch::Aarch64) => bail!("aarch64 is not supported for FreeBSD"),
            (Os::FreeBsd, Arch::Armv7L) => bail!("armv7l is not supported for FreeBSD"),
            (Os::FreeBsd, Arch::X86) => bail!("32-bit wheels are not supported for FreeBSD"),
            (Os::FreeBsd, Arch::X86_64) => {
                match PlatformInfo::new() {
                    Ok(_) => {}
                    Err(error) => bail!(error),
                };
            }
            (Os::Macos, Arch::Armv7L) => bail!("armv7l is not supported for macOS"),
            (Os::Macos, Arch::X86) => bail!("32-bit wheels are not supported for macOS"),
            (Os::Windows, Arch::Aarch64) => bail!("aarch64 is not supported for Windows"),
            (Os::Windows, Arch::Armv7L) => bail!("armv7l is not supported for Windows"),
            (_, _) => {}
        }
        Ok(Target {
            os,
            arch,
            env: platform.target_env,
            triple: platform.target_triple,
        })
    }

    /// Returns whether the platform is 64 bit or 32 bit
    pub fn pointer_width(&self) -> usize {
        match self.arch {
            Arch::Aarch64 => 64,
            Arch::Armv7L => 32,
            Arch::Powerpc64 => 64,
            Arch::Powerpc64Le => 64,
            Arch::X86 => 32,
            Arch::X86_64 => 64,
        }
    }

    /// Returns target architecture
    pub fn target_arch(&self) -> Arch {
        self.arch
    }

    /// Returns target triple string
    pub fn target_triple(&self) -> &str {
        &self.triple
    }

    /// Returns true if the current platform is linux or mac os
    pub fn is_unix(&self) -> bool {
        self.os != Os::Windows
    }

    /// Returns true if the current platform is linux
    pub fn is_linux(&self) -> bool {
        self.os == Os::Linux
    }

    /// Returns true if the current platform is freebsd
    pub fn is_freebsd(&self) -> bool {
        self.os == Os::FreeBsd
    }

    /// Returns true if the current platform is mac os
    pub fn is_macos(&self) -> bool {
        self.os == Os::Macos
    }

    /// Returns true if the current platform is windows
    pub fn is_windows(&self) -> bool {
        self.os == Os::Windows
    }

    /// Returns true if the current platform's target env is Musl
    pub fn is_musl_target(&self) -> bool {
        match self.env {
            Some(Env::Musl) => true,
            Some(_) => false,
            None => false,
        }
    }

    /// Returns the default Manylinux tag for this architecture
    pub fn get_default_manylinux_tag(&self) -> Manylinux {
        match self.arch {
            Arch::Aarch64 | Arch::Armv7L | Arch::Powerpc64 | Arch::Powerpc64Le => {
                Manylinux::Manylinux2014
            }
            Arch::X86 | Arch::X86_64 => Manylinux::Manylinux2010,
        }
    }

    /// Returns the platform part of the tag for the wheel name for cffi wheels
    pub fn get_platform_tag(&self, manylinux: &Manylinux, universal2: bool) -> String {
        match (&self.os, &self.arch) {
            (Os::FreeBsd, Arch::X86_64) => {
                let info = match PlatformInfo::new() {
                    Ok(info) => info,
                    Err(error) => panic!("{}", error),
                };
                let release = info.release().replace(".", "_").replace("-", "_");
                format!("freebsd_{}_amd64", release)
            }
            (Os::Linux, _) => format!("{}_{}", manylinux, self.arch),
            (Os::Macos, Arch::X86_64) => {
                if universal2 {
                    "macosx_10_9_universal2".to_string()
                } else {
                    "macosx_10_7_x86_64".to_string()
                }
            }
            (Os::Macos, Arch::Aarch64) => {
                if universal2 {
                    "macosx_10_9_universal2".to_string()
                } else {
                    "macosx_11_0_arm64".to_string()
                }
            }
            (Os::Windows, Arch::X86) => "win32".to_string(),
            (Os::Windows, Arch::X86_64) => "win_amd64".to_string(),
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
            (Os::FreeBsd, _) => "", // according imp.get_suffixes(), there are no such
            (Os::Linux, Arch::Aarch64) => "aarch64-linux-gnu", // aka armv8-linux-gnueabihf
            (Os::Linux, Arch::Armv7L) => "arm-linux-gnueabihf",
            (Os::Linux, Arch::Powerpc64) => "powerpc64-linux-gnu",
            (Os::Linux, Arch::Powerpc64Le) => "powerpc64le-linux-gnu",
            (Os::Linux, Arch::X86) => "i386-linux-gnu", // not i686
            (Os::Linux, Arch::X86_64) => "x86_64-linux-gnu",
            (Os::Macos, Arch::X86_64) => "darwin",
            (Os::Macos, Arch::Aarch64) => "darwin",
            (Os::Windows, Arch::X86) => "win32",
            (Os::Windows, Arch::X86_64) => "win_amd64",
            (Os::Macos, _) => {
                panic!("unsupported macOS Arch should not have reached get_shared_platform_tag()")
            }
            (Os::Windows, _) => {
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

    /// Returns the site-packages directory inside a venv e.g.
    /// {venv_base}/lib/python{x}.{y} on unix or {venv_base}/Lib on window
    pub fn get_venv_site_package(
        &self,
        venv_base: impl AsRef<Path>,
        interpreter: &PythonInterpreter,
    ) -> PathBuf {
        if self.is_unix() {
            let python_dir = format!("python{}.{}", interpreter.major, interpreter.minor);

            venv_base
                .as_ref()
                .join("lib")
                .join(python_dir)
                .join("site-packages")
        } else {
            venv_base.as_ref().join("Lib").join("site-packages")
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
