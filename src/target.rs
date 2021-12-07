use crate::cross_compile::is_cross_compiling;
use crate::python_interpreter::InterpreterKind;
use crate::{PlatformTag, PythonInterpreter};
use anyhow::{bail, format_err, Context, Result};
use platform_info::*;
use std::env;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str;
use target_lexicon::{Environment, Triple};

/// All supported operating system
#[derive(Debug, Clone, Eq, PartialEq)]
enum Os {
    Linux,
    Windows,
    Macos,
    FreeBsd,
    NetBsd,
    OpenBsd,
    Dragonfly,
    Illumos,
    Haiku,
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Os::Linux => write!(f, "Linux"),
            Os::Windows => write!(f, "Windows"),
            Os::Macos => write!(f, "MacOS"),
            Os::FreeBsd => write!(f, "FreeBSD"),
            Os::NetBsd => write!(f, "NetBSD"),
            Os::OpenBsd => write!(f, "OpenBSD"),
            Os::Dragonfly => write!(f, "DragonFly"),
            Os::Illumos => write!(f, "Illumos"),
            Os::Haiku => write!(f, "Haiku"),
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
            Arch::S390X,
            Arch::X86,
            Arch::X86_64,
        ],
        Os::Windows => vec![Arch::X86, Arch::X86_64, Arch::Aarch64],
        Os::Macos => vec![Arch::Aarch64, Arch::X86_64],
        Os::NetBsd => vec![Arch::Aarch64, Arch::X86, Arch::X86_64],
        Os::FreeBsd => vec![
            Arch::Aarch64,
            Arch::Powerpc64,
            Arch::Powerpc64Le,
            Arch::X86_64,
        ],
        Os::OpenBsd => vec![Arch::X86, Arch::X86_64, Arch::Aarch64],
        Os::Dragonfly => vec![Arch::X86_64],
        Os::Illumos => vec![Arch::X86_64],
        Os::Haiku => vec![Arch::X86_64],
    }
}

/// The part of the current platform that is relevant when building wheels and is supported
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Target {
    os: Os,
    arch: Arch,
    env: Environment,
    triple: String,
    cross_compiling: bool,
}

impl Target {
    /// Uses the given target triple or tries the guess the current target by using the one used
    /// for compilation
    ///
    /// Fails if the target triple isn't supported
    pub fn from_target_triple(target_triple: Option<String>) -> Result<Self> {
        let (platform, triple) = if let Some(ref target_triple) = target_triple {
            let platform: Triple = target_triple
                .parse()
                .map_err(|_| format_err!("Unknown target triple {}", target_triple))?;
            (platform, target_triple.to_string())
        } else {
            let target_triple = get_host_target()?;
            let platform: Triple = target_triple
                .parse()
                .map_err(|_| format_err!("Unknown target triple {}", target_triple))?;
            (platform, target_triple)
        };

        let os = match platform.operating_system {
            target_lexicon::OperatingSystem::Linux => Os::Linux,
            target_lexicon::OperatingSystem::Windows => Os::Windows,
            target_lexicon::OperatingSystem::MacOSX { .. }
            | target_lexicon::OperatingSystem::Darwin => Os::Macos,
            target_lexicon::OperatingSystem::Netbsd => Os::NetBsd,
            target_lexicon::OperatingSystem::Freebsd => Os::FreeBsd,
            target_lexicon::OperatingSystem::Openbsd => Os::OpenBsd,
            target_lexicon::OperatingSystem::Dragonfly => Os::Dragonfly,
            target_lexicon::OperatingSystem::Illumos => Os::Illumos,
            target_lexicon::OperatingSystem::Haiku => Os::Haiku,
            unsupported => bail!("The operating system {:?} is not supported", unsupported),
        };

        let arch = match platform.architecture {
            target_lexicon::Architecture::X86_64 => Arch::X86_64,
            target_lexicon::Architecture::X86_32(_) => Arch::X86,
            target_lexicon::Architecture::Arm(_) => Arch::Armv7L,
            target_lexicon::Architecture::Aarch64(_) => Arch::Aarch64,
            target_lexicon::Architecture::Powerpc64 => Arch::Powerpc64,
            target_lexicon::Architecture::Powerpc64le => Arch::Powerpc64Le,
            target_lexicon::Architecture::S390x => Arch::S390X,
            unsupported => bail!("The architecture {} is not supported", unsupported),
        };

        if !get_supported_architectures(&os).contains(&arch) {
            bail!("{} is not supported on {}", arch, os);
        }

        let mut target = Target {
            os,
            arch,
            env: platform.environment,
            triple,
            cross_compiling: false,
        };
        target.cross_compiling = is_cross_compiling(&target)?;
        Ok(target)
    }

    /// Returns the platform part of the tag for the wheel name
    pub fn get_platform_tag(&self, platform_tag: PlatformTag, universal2: bool) -> Result<String> {
        let tag = match (&self.os, &self.arch) {
            // FreeBSD
            (Os::FreeBsd, Arch::X86_64)
            | (Os::FreeBsd, Arch::Aarch64)
            | (Os::FreeBsd, Arch::Powerpc64)
            | (Os::FreeBsd, Arch::Powerpc64Le)
            // NetBSD
            | (Os::NetBsd, Arch::X86)
            | (Os::NetBsd, Arch::X86_64)
            | (Os::NetBsd, Arch::Aarch64)
            // OpenBSD
            | (Os::OpenBsd, Arch::X86)
            | (Os::OpenBsd, Arch::X86_64)
            | (Os::OpenBsd, Arch::Aarch64) => {
                let info = PlatformInfo::new()?;
                let release = info.release().replace(".", "_").replace("-", "_");
                let arch = match self.arch {
                    Arch::X86_64 => "amd64",
                    Arch::X86 => "i386",
                    Arch::Aarch64 => "arm64",
                    Arch::Powerpc64 => "powerpc64",
                    Arch::Powerpc64Le => "powerpc64le",
                    _ => panic!(
                        "unsupported architecture should not have reached get_platform_tag()"
                    ),
                };
                format!(
                    "{}_{}_{}",
                    self.os.to_string().to_ascii_lowercase(),
                    release,
                    arch
                )
            }
            // DragonFly
            (Os::Dragonfly, Arch::X86_64)
            // Haiku
            | (Os::Haiku, Arch::X86_64) => {
                let info = PlatformInfo::new()?;
                let release = info.release().replace(".", "_").replace("-", "_");
                format!(
                    "{}_{}_{}",
                    self.os.to_string().to_ascii_lowercase(),
                    release.to_ascii_lowercase(),
                    "x86_64"
                )
            }
            (Os::Illumos, Arch::X86_64) => {
                let info = PlatformInfo::new()?;
                let mut release = info.release().replace(".", "_").replace("-", "_");
                let mut arch = info.machine().replace(' ', "_").replace('/', "_");

                let mut os = self.os.to_string().to_ascii_lowercase();
                // See https://github.com/python/cpython/blob/46c8d915715aa2bd4d697482aa051fe974d440e1/Lib/sysconfig.py#L722-L730
                if let Some((major, other)) = release.split_once('_') {
                    let major_ver: u64 = major.parse().context("illumos major version is not a number")?;
                    if major_ver >= 5 {
                        // SunOS 5 == Solaris 2
                        os = "solaris".to_string();
                        release = format!("{}_{}", major_ver - 3, other);
                        arch = format!("{}_64bit", arch);
                    }
                }
                format!(
                    "{}_{}_{}",
                    os,
                    release,
                    arch
                )
            }
            (Os::Linux, _) => {
                let arch = if self.cross_compiling {
                    self.arch.to_string()
                } else {
                    PlatformInfo::new()
                        .map(|info| info.machine().into_owned())
                        .unwrap_or_else(|_| self.arch.to_string())
                };
                let mut tags = vec![format!("{}_{}", platform_tag, arch)];
                for alias in platform_tag.aliases() {
                    tags.push(format!("{}_{}", alias, arch));
                }
                tags.join(".")
            }
            (Os::Macos, Arch::X86_64) => {
                let ((x86_64_major, x86_64_minor), (arm64_major, arm64_minor)) = macosx_deployment_target(env::var("MACOSX_DEPLOYMENT_TARGET").ok().as_deref(), universal2)?;
                if universal2 {
                    format!(
                        "macosx_{x86_64_major}_{x86_64_minor}_x86_64.macosx_{arm64_major}_{arm64_minor}_arm64.macosx_{x86_64_major}_{x86_64_minor}_universal2",
                        x86_64_major = x86_64_major,
                        x86_64_minor = x86_64_minor,
                        arm64_major = arm64_major,
                        arm64_minor = arm64_minor
                    )
                } else {
                    format!("macosx_{}_{}_x86_64", x86_64_major, x86_64_minor)
                }
            }
            (Os::Macos, Arch::Aarch64) => {
                let ((x86_64_major, x86_64_minor), (arm64_major, arm64_minor)) = macosx_deployment_target(env::var("MACOSX_DEPLOYMENT_TARGET").ok().as_deref(), universal2)?;
                if universal2 {
                    format!(
                        "macosx_{x86_64_major}_{x86_64_minor}_x86_64.macosx_{arm64_major}_{arm64_minor}_arm64.macosx_{x86_64_major}_{x86_64_minor}_universal2",
                        x86_64_major = x86_64_major,
                        x86_64_minor = x86_64_minor,
                        arm64_major = arm64_major,
                        arm64_minor = arm64_minor
                    )
                } else {
                    format!("macosx_{}_{}_arm64", arm64_major, arm64_minor)
                }
            }
            (Os::Windows, Arch::X86) => "win32".to_string(),
            (Os::Windows, Arch::X86_64) => "win_amd64".to_string(),
            (Os::Windows, Arch::Aarch64) => "win_arm64".to_string(),
            (_, _) => panic!("unsupported target should not have reached get_platform_tag()"),
        };
        Ok(tag)
    }

    /// Returns the name python uses in `sys.platform` for this os
    pub fn get_python_os(&self) -> &str {
        match self.os {
            Os::Windows => "windows",
            Os::Linux => "linux",
            Os::Macos => "darwin",
            Os::FreeBsd => "freebsd",
            Os::NetBsd => "netbsd",
            Os::OpenBsd => "openbsd",
            Os::Dragonfly => "dragonfly",
            Os::Illumos => "sunos",
            Os::Haiku => "haiku",
        }
    }

    /// Returns the default Manylinux tag for this architecture
    pub fn get_default_manylinux_tag(&self) -> PlatformTag {
        match self.arch {
            Arch::Aarch64 | Arch::Armv7L | Arch::Powerpc64 | Arch::Powerpc64Le | Arch::S390X => {
                PlatformTag::manylinux2014()
            }
            Arch::X86 | Arch::X86_64 => PlatformTag::manylinux2010(),
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

    /// Returns target triple string
    pub fn target_triple(&self) -> &str {
        &self.triple
    }

    /// Returns true if the current platform is not windows
    pub fn is_unix(&self) -> bool {
        match self.os {
            Os::Windows => false,
            Os::Linux
            | Os::Macos
            | Os::FreeBsd
            | Os::NetBsd
            | Os::OpenBsd
            | Os::Dragonfly
            | Os::Illumos
            | Os::Haiku => true,
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

    /// Returns true if the current platform is illumos
    pub fn is_illumos(&self) -> bool {
        self.os == Os::Illumos
    }

    /// Returns true if the current platform is haiku
    pub fn is_haiku(&self) -> bool {
        self.os == Os::Haiku
    }

    /// Returns true if the current platform's target env is Musl
    pub fn is_musl_target(&self) -> bool {
        matches!(
            self.env,
            Environment::Musl
                | Environment::Musleabi
                | Environment::Musleabihf
                | Environment::Muslabi64
        )
    }

    /// Is cross compiling for this target
    pub fn cross_compiling(&self) -> bool {
        self.cross_compiling
    }

    /// Returns the tags for the WHEEL file for cffi wheels
    pub fn get_py3_tags(&self, platform_tag: PlatformTag, universal2: bool) -> Result<Vec<String>> {
        let tags = vec![format!(
            "py3-none-{}",
            self.get_platform_tag(platform_tag, universal2)?
        )];
        Ok(tags)
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
            match interpreter.interpreter_kind {
                InterpreterKind::CPython => {
                    let python_dir = format!("python{}.{}", interpreter.major, interpreter.minor);
                    venv_base
                        .as_ref()
                        .join("lib")
                        .join(python_dir)
                        .join("site-packages")
                }
                InterpreterKind::PyPy => venv_base.as_ref().join("site-packages"),
            }
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
        platform_tag: PlatformTag,
        universal2: bool,
    ) -> Result<(String, Vec<String>)> {
        let tag = format!(
            "py3-none-{platform}",
            platform = self.get_platform_tag(platform_tag, universal2)?
        );
        let tags = self.get_py3_tags(platform_tag, universal2)?;
        Ok((tag, tags))
    }
}

pub(crate) fn get_host_target() -> Result<String> {
    let output = Command::new("rustc").arg("-vV").output();
    let output = match output {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "rustc, the rust compiler, is not installed or not in PATH. \
                This package requires Rust and Cargo to compile extensions. \
                Install it through the system's package manager or via https://rustup.rs/.",
            );
        }
        Err(err) => {
            return Err(err).context("Failed to run rustc to get the host target");
        }
        Ok(output) => output,
    };

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

fn macosx_deployment_target(
    deploy_target: Option<&str>,
    universal2: bool,
) -> Result<((usize, usize), (usize, usize))> {
    let x86_64_default = if universal2 { (10, 9) } else { (10, 7) };
    let arm64_default = (11, 0);
    let mut x86_64_ver = x86_64_default;
    let mut arm64_ver = arm64_default;
    if let Some(deploy_target) = deploy_target {
        let err_ctx = "MACOSX_DEPLOYMENT_TARGET is invalid";
        let mut parts = deploy_target.split('.');
        let major = parts.next().context(err_ctx)?;
        let major: usize = major.parse().context(err_ctx)?;
        let minor = parts.next().context(err_ctx)?;
        let minor: usize = minor.parse().context(err_ctx)?;
        if (major, minor) > x86_64_default {
            x86_64_ver = (major, minor);
        }
        if (major, minor) > arm64_default {
            arm64_ver = (major, minor);
        }
    }
    Ok((x86_64_ver, arm64_ver))
}

#[cfg(test)]
mod test {
    use super::macosx_deployment_target;

    #[test]
    fn test_macosx_deployment_target() {
        assert_eq!(
            macosx_deployment_target(None, false).unwrap(),
            (((10, 7), (11, 0)))
        );
        assert_eq!(
            macosx_deployment_target(None, true).unwrap(),
            (((10, 9), (11, 0)))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.6"), false).unwrap(),
            (((10, 7), (11, 0)))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.6"), true).unwrap(),
            (((10, 9), (11, 0)))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.9"), false).unwrap(),
            (((10, 9), (11, 0)))
        );
        assert_eq!(
            macosx_deployment_target(Some("11.0.0"), false).unwrap(),
            (((11, 0), (11, 0)))
        );
        assert_eq!(
            macosx_deployment_target(Some("11.1"), false).unwrap(),
            (((11, 1), (11, 1)))
        );
    }
}
