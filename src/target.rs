use std::env;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str;

use anyhow::{bail, format_err, Context, Result};
use platform_info::*;
use platforms::target::Env;
use platforms::Platform;

use crate::{Manylinux, PythonInterpreter};

/// All supported operating system
#[derive(Debug, Clone, Eq, PartialEq)]
enum Os {
    Linux,
    Windows,
    Macos,
    FreeBsd,
    OpenBsd,
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Os::Linux => write!(f, "Linux"),
            Os::Windows => write!(f, "Windows"),
            Os::Macos => write!(f, "MacOS"),
            Os::FreeBsd => write!(f, "FreeBSD"),
            Os::OpenBsd => write!(f, "OpenBSD"),
        }
    }
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
    S390X,
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
            Arch::S390X => write!(f, "s390x"),
        }
    }
}

// Returns the set of supported architectures for each operating system
fn get_supported_architectures(os: &Os) -> Vec<Arch> {
    match os {
        Os::Linux => vec![
            Arch::Aarch64,
            Arch::Armv7L,
            Arch::Powerpc64,
            Arch::Powerpc64Le,
            Arch::X86,
            Arch::X86_64,
        ],
        Os::Windows => vec![Arch::X86, Arch::X86_64],
        Os::Macos => vec![Arch::Aarch64, Arch::X86_64],
        Os::FreeBsd => vec![Arch::X86_64],
        Os::OpenBsd => vec![Arch::X86_64],
    }
}

/// The part of the current platform that is relevant when building wheels and is supported
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Target {
    os: Os,
    arch: Arch,
    env: Option<Env>,
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
            let target_triple = get_host_target()?;
            Platform::find(&target_triple)
                .ok_or_else(|| format_err!("Unknown target triple {}", target_triple))?
        };

        let os = match platform.target_os {
            platforms::target::OS::Linux => Os::Linux,
            platforms::target::OS::Windows => Os::Windows,
            platforms::target::OS::MacOS => Os::Macos,
            platforms::target::OS::FreeBSD => Os::FreeBsd,
            platforms::target::OS::OpenBSD => Os::OpenBsd,
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
            unsupported => bail!("The architecture {} is not supported", unsupported),
        };

        if !get_supported_architectures(&os).contains(&arch) {
            bail!("{} is not supported on {}", arch, os);
        }

        Ok(Target {
            os,
            arch,
            env: platform.target_env,
        })
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
            (Os::OpenBsd, Arch::X86_64) => {
                let info = match PlatformInfo::new() {
                    Ok(info) => info,
                    Err(error) => panic!("{}", error),
                };
                let release = info.release().replace(".", "_").replace("-", "_");
                format!("openbsd_{}_amd64", release)
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

    /// Returns the name python uses in `sys.platform` for this os
    pub fn get_python_os(&self) -> &str {
        match self.os {
            Os::Windows => "windows",
            Os::Linux => "linux",
            Os::Macos => "darwin",
            Os::FreeBsd => "freebsd",
            Os::OpenBsd => "openbsd",
        }
    }

    /// Returns the default Manylinux tag for this architecture
    pub fn get_default_manylinux_tag(&self) -> Manylinux {
        match self.arch {
            Arch::Aarch64 | Arch::Armv7L | Arch::Powerpc64 | Arch::Powerpc64Le | Arch::S390X => {
                Manylinux::Manylinux2014
            }
            Arch::X86 | Arch::X86_64 => Manylinux::Manylinux2010,
        }
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
            Arch::S390X => 64,
        }
    }

    /// Returns true if the current platform is not windows
    pub fn is_unix(&self) -> bool {
        match self.os {
            Os::Windows => false,
            Os::Linux | Os::Macos | Os::FreeBsd | Os::OpenBsd => true,
        }
    }

    /// Returns target architecture
    pub fn target_arch(&self) -> Arch {
        self.arch
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

    /// Returns the tags for the WHEEL file for cffi wheels
    pub fn get_py3_tags(&self, manylinux: &Manylinux, universal2: bool) -> Vec<String> {
        vec![format!(
            "py3-none-{}",
            self.get_platform_tag(&manylinux, universal2)
        )]
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

fn get_host_target() -> Result<String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("Failed to run rustc to get the host target")?;
    let output = str::from_utf8(&output.stdout).context("`rustc -vV` didn't return utf8 output")?;

    let field = "host: ";
    let host = output
        .lines()
        .find(|l| l.starts_with(field))
        .map(|l| &l[field.len()..])
        .ok_or_else(|| {
            format_err!(
                "`rustc -vV` didn't have a line for `{}`, got:\n{}",
                field.trim(),
                output
            )
        })?
        .to_string();
    Ok(host)
}
