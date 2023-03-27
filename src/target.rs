use crate::cross_compile::is_cross_compiling;
use crate::PlatformTag;
use anyhow::{anyhow, bail, format_err, Result};
use platform_info::*;
use rustc_version::VersionMeta;
use serde::Deserialize;
use std::env;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::str;
use target_lexicon::{Architecture, Environment, Triple};
use tracing::error;

pub(crate) const RUST_1_64_0: semver::Version = semver::Version::new(1, 64, 0);

/// All supported operating system
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Windows,
    Macos,
    FreeBsd,
    NetBsd,
    OpenBsd,
    Dragonfly,
    Solaris,
    Illumos,
    Haiku,
    Emscripten,
    Wasi,
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Os::Linux => write!(f, "Linux"),
            Os::Windows => write!(f, "Windows"),
            Os::Macos => write!(f, "macOS"),
            Os::FreeBsd => write!(f, "FreeBSD"),
            Os::NetBsd => write!(f, "NetBSD"),
            Os::OpenBsd => write!(f, "OpenBSD"),
            Os::Dragonfly => write!(f, "DragonFly"),
            Os::Solaris => write!(f, "Solaris"),
            Os::Illumos => write!(f, "Illumos"),
            Os::Haiku => write!(f, "Haiku"),
            Os::Emscripten => write!(f, "Emscripten"),
            Os::Wasi => write!(f, "Wasi"),
        }
    }
}

/// All supported CPU architectures
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    Aarch64,
    Armv6L,
    Armv7L,
    #[serde(alias = "ppc")]
    Powerpc,
    #[serde(alias = "ppc64le")]
    Powerpc64Le,
    #[serde(alias = "ppc64")]
    Powerpc64,
    #[serde(alias = "i686")]
    X86,
    X86_64,
    S390X,
    Wasm32,
    Riscv64,
    Mips64el,
    Mipsel,
    Sparc64,
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Arch::Aarch64 => write!(f, "aarch64"),
            Arch::Armv6L => write!(f, "armv6l"),
            Arch::Armv7L => write!(f, "armv7l"),
            Arch::Powerpc => write!(f, "ppc"),
            Arch::Powerpc64Le => write!(f, "ppc64le"),
            Arch::Powerpc64 => write!(f, "ppc64"),
            Arch::X86 => write!(f, "i686"),
            Arch::X86_64 => write!(f, "x86_64"),
            Arch::S390X => write!(f, "s390x"),
            Arch::Wasm32 => write!(f, "wasm32"),
            Arch::Riscv64 => write!(f, "riscv64"),
            Arch::Mips64el => write!(f, "mips64el"),
            Arch::Mipsel => write!(f, "mipsel"),
            Arch::Sparc64 => write!(f, "sparc64"),
        }
    }
}

impl Arch {
    /// Represents the hardware platform.
    ///
    /// This is the same as the native platform's `uname -m` output.
    pub fn machine(&self) -> &'static str {
        // See https://www.freebsd.org/cgi/man.cgi?query=arch&sektion=7&format=html
        // MACHINE_ARCH	vs MACHINE_CPUARCH vs MACHINE section
        match self {
            Arch::Aarch64 => "arm64",
            Arch::Armv6L | Arch::Armv7L => "arm",
            Arch::Powerpc | Arch::Powerpc64Le | Arch::Powerpc64 => "powerpc",
            Arch::X86 => "i386",
            Arch::X86_64 => "amd64",
            Arch::Riscv64 => "riscv",
            Arch::Mips64el | Arch::Mipsel => "mips",
            // sparc64 is unsupported since FreeBSD 13.0
            Arch::Sparc64 => "sparc64",
            Arch::Wasm32 => "wasm32",
            Arch::S390X => "s390x",
        }
    }
}

// Returns the set of supported architectures for each operating system
fn get_supported_architectures(os: &Os) -> Vec<Arch> {
    match os {
        Os::Linux => vec![
            Arch::Aarch64,
            Arch::Armv6L,
            Arch::Armv7L,
            Arch::Powerpc,
            Arch::Powerpc64,
            Arch::Powerpc64Le,
            Arch::S390X,
            Arch::X86,
            Arch::X86_64,
            Arch::Riscv64,
            Arch::Mips64el,
            Arch::Mipsel,
            Arch::Sparc64,
        ],
        Os::Windows => vec![Arch::X86, Arch::X86_64, Arch::Aarch64],
        Os::Macos => vec![Arch::Aarch64, Arch::X86_64],
        Os::FreeBsd | Os::NetBsd => vec![
            Arch::Aarch64,
            Arch::Armv6L,
            Arch::Armv7L,
            Arch::Powerpc,
            Arch::Powerpc64,
            Arch::Powerpc64Le,
            Arch::X86,
            Arch::X86_64,
            Arch::Riscv64,
            Arch::Mips64el,
            Arch::Mipsel,
            Arch::Sparc64,
        ],
        Os::OpenBsd => vec![
            Arch::X86,
            Arch::X86_64,
            Arch::Aarch64,
            Arch::Armv7L,
            Arch::Powerpc,
            Arch::Powerpc64,
            Arch::Powerpc64Le,
            Arch::Riscv64,
            Arch::Sparc64,
        ],
        Os::Dragonfly => vec![Arch::X86_64],
        Os::Illumos => vec![Arch::X86_64],
        Os::Haiku => vec![Arch::X86_64],
        Os::Solaris => vec![Arch::X86_64, Arch::Sparc64],
        Os::Emscripten | Os::Wasi => vec![Arch::Wasm32],
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
    /// rustc version information
    pub(crate) rustc_version: VersionMeta,
    /// Is user specified `--target`
    pub(crate) user_specified: bool,
}

impl Target {
    /// Uses the given target triple or tries the guess the current target by using the one used
    /// for compilation
    ///
    /// Fails if the target triple isn't supported
    pub fn from_target_triple(target_triple: Option<String>) -> Result<Self> {
        use target_lexicon::{
            ArmArchitecture, Mips32Architecture, Mips64Architecture, OperatingSystem,
        };

        let rustc_version = rustc_version_meta()?;
        let host_triple = &rustc_version.host;
        let (platform, triple) = if let Some(ref target_triple) = target_triple {
            let platform: Triple = target_triple
                .parse()
                .map_err(|_| format_err!("Unknown target triple {}", target_triple))?;
            (platform, target_triple.to_string())
        } else {
            let platform: Triple = host_triple
                .parse()
                .map_err(|_| format_err!("Unknown target triple {}", host_triple))?;
            (platform, host_triple.clone())
        };

        let os = match platform.operating_system {
            OperatingSystem::Linux => Os::Linux,
            OperatingSystem::Windows => Os::Windows,
            OperatingSystem::MacOSX { .. } | OperatingSystem::Darwin => Os::Macos,
            OperatingSystem::Netbsd => Os::NetBsd,
            OperatingSystem::Freebsd => Os::FreeBsd,
            OperatingSystem::Openbsd => Os::OpenBsd,
            OperatingSystem::Dragonfly => Os::Dragonfly,
            OperatingSystem::Solaris => Os::Solaris,
            OperatingSystem::Illumos => Os::Illumos,
            OperatingSystem::Haiku => Os::Haiku,
            OperatingSystem::Emscripten => Os::Emscripten,
            OperatingSystem::Wasi => Os::Wasi,
            unsupported => bail!("The operating system {:?} is not supported", unsupported),
        };

        let arch = match platform.architecture {
            Architecture::X86_64 => Arch::X86_64,
            Architecture::X86_32(_) => Arch::X86,
            Architecture::Arm(arm_arch) => match arm_arch {
                ArmArchitecture::Arm | ArmArchitecture::Armv6 => Arch::Armv6L,
                _ => Arch::Armv7L,
            },
            Architecture::Aarch64(_) => Arch::Aarch64,
            Architecture::Powerpc => Arch::Powerpc,
            Architecture::Powerpc64 => Arch::Powerpc64,
            Architecture::Powerpc64le => Arch::Powerpc64Le,
            Architecture::S390x => Arch::S390X,
            Architecture::Wasm32 => Arch::Wasm32,
            Architecture::Riscv64(_) => Arch::Riscv64,
            Architecture::Mips64(Mips64Architecture::Mips64el) => Arch::Mips64el,
            Architecture::Mips32(Mips32Architecture::Mipsel) => Arch::Mipsel,
            Architecture::Sparc64 => Arch::Sparc64,
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
            rustc_version,
            user_specified: target_triple.is_some(),
            cross_compiling: false,
        };
        target.cross_compiling = is_cross_compiling(&target)?;
        Ok(target)
    }

    /// Returns the platform architecture
    pub fn get_platform_arch(&self) -> Result<String> {
        if self.cross_compiling {
            return Ok(self.arch.to_string());
        }
        let machine = PlatformInfo::new().map(|info| info.machine().into_owned());
        let arch = match machine {
            Ok(machine) => {
                let linux32 = (machine == "x86_64" && self.arch != Arch::X86_64)
                    || (machine == "aarch64" && self.arch != Arch::Aarch64);
                if linux32 {
                    // When running in Docker sometimes uname returns 64-bit architecture while the container is actually 32-bit,
                    // In this case we trust the architecture of rustc target
                    self.arch.to_string()
                } else {
                    machine
                }
            }
            Err(err) => {
                error!("Failed to get machine architecture: {}", err);
                self.arch.to_string()
            }
        };
        Ok(arch)
    }

    /// Returns the platform release
    pub fn get_platform_release(&self) -> Result<String> {
        let os = self.os.to_string();
        let os_version = env::var(format!("MATURIN_{}_VERSION", os.to_ascii_uppercase()));
        let release = match os_version {
            Ok(os_ver) => os_ver,
            Err(_) => {
                let info = PlatformInfo::new()?;
                info.release().to_string()
            }
        };
        let release = release.replace(['.', '-'], "_");
        Ok(release)
    }

    /// Returns the name python uses in `sys.platform` for this architecture.
    pub fn get_python_arch(&self) -> &str {
        match self.arch {
            Arch::Aarch64 => "aarch64",
            Arch::Armv6L => "armv6l",
            Arch::Armv7L => "armv7l",
            Arch::Powerpc => "ppc",
            Arch::Powerpc64Le => "powerpc64le",
            Arch::Powerpc64 => "powerpc64",
            Arch::X86 => "i386",
            Arch::X86_64 => "x86_64",
            Arch::S390X => "s390x",
            Arch::Wasm32 => "wasm32",
            Arch::Riscv64 => "riscv64",
            // It's kinda surprising that Python doesn't include the `el` suffix
            Arch::Mips64el => "mips64",
            Arch::Mipsel => "mips",
            Arch::Sparc64 => "sparc64",
        }
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
            Os::Solaris => "sunos",
            Os::Illumos => "sunos",
            Os::Haiku => "haiku",
            Os::Emscripten => "emscripten",
            // This isn't real, there's no sys.platform here
            Os::Wasi => "wasi",
        }
    }

    /// Returns the oldest possible Manylinux tag for this architecture
    pub fn get_minimum_manylinux_tag(&self) -> PlatformTag {
        match self.arch {
            Arch::Aarch64 | Arch::Armv7L | Arch::Powerpc64 | Arch::Powerpc64Le | Arch::S390X => {
                PlatformTag::manylinux2014()
            }
            Arch::X86 | Arch::X86_64 => {
                // rustc 1.64.0 bumps glibc requirement to 2.17
                // see https://blog.rust-lang.org/2022/08/01/Increasing-glibc-kernel-requirements.html
                if self.rustc_version.semver >= RUST_1_64_0 {
                    PlatformTag::manylinux2014()
                } else {
                    PlatformTag::manylinux2010()
                }
            }
            Arch::Armv6L
            | Arch::Wasm32
            | Arch::Riscv64
            | Arch::Mips64el
            | Arch::Mipsel
            | Arch::Powerpc
            | Arch::Sparc64 => PlatformTag::Linux,
        }
    }

    /// Returns whether the platform is 64 bit or 32 bit
    pub fn pointer_width(&self) -> usize {
        match self.arch {
            Arch::Aarch64
            | Arch::Powerpc64
            | Arch::Powerpc64Le
            | Arch::X86_64
            | Arch::S390X
            | Arch::Riscv64
            | Arch::Mips64el
            | Arch::Sparc64 => 64,
            Arch::Armv6L
            | Arch::Armv7L
            | Arch::X86
            | Arch::Wasm32
            | Arch::Mipsel
            | Arch::Powerpc => 32,
        }
    }

    /// Returns target triple as string
    #[inline]
    pub fn target_triple(&self) -> &str {
        &self.triple
    }

    /// Returns host triple as string
    #[inline]
    pub fn host_triple(&self) -> &str {
        &self.rustc_version.host
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
            | Os::Solaris
            | Os::Illumos
            | Os::Haiku
            | Os::Emscripten
            | Os::Wasi => true,
        }
    }

    /// Returns target operating system
    #[inline]
    pub fn target_os(&self) -> Os {
        self.os
    }

    /// Returns target architecture
    #[inline]
    pub fn target_arch(&self) -> Arch {
        self.arch
    }

    /// Returns target environment
    #[inline]
    pub fn target_env(&self) -> Environment {
        self.env
    }

    /// Returns true if the current platform is linux
    #[inline]
    pub fn is_linux(&self) -> bool {
        self.os == Os::Linux
    }

    /// Returns true if the current platform is freebsd
    #[inline]
    pub fn is_freebsd(&self) -> bool {
        self.os == Os::FreeBsd
    }

    /// Returns true if the current platform is mac os
    #[inline]
    pub fn is_macos(&self) -> bool {
        self.os == Os::Macos
    }

    /// Returns true if the current platform is windows
    #[inline]
    pub fn is_windows(&self) -> bool {
        self.os == Os::Windows
    }

    /// Returns true if the current environment is msvc
    #[inline]
    pub fn is_msvc(&self) -> bool {
        self.env == Environment::Msvc
    }

    /// Returns true if the current platform is illumos
    #[inline]
    pub fn is_illumos(&self) -> bool {
        self.os == Os::Illumos
    }

    /// Returns true if the current platform is haiku
    #[inline]
    pub fn is_haiku(&self) -> bool {
        self.os == Os::Haiku
    }

    /// Returns true if the current platform is Emscripten
    #[inline]
    pub fn is_emscripten(&self) -> bool {
        self.os == Os::Emscripten
    }

    /// Returns true if we're building a binary for wasm32-wasi
    #[inline]
    pub fn is_wasi(&self) -> bool {
        self.os == Os::Wasi
    }

    /// Returns true if the current platform's target env is Musl
    #[inline]
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
    #[inline]
    pub fn cross_compiling(&self) -> bool {
        self.cross_compiling
    }

    /// Returns the path to the python executable inside a venv
    pub fn get_venv_python(&self, venv_base: impl AsRef<Path>) -> PathBuf {
        let python = if self.is_windows() {
            "python.exe"
        } else {
            "python"
        };
        self.get_venv_bin_dir(venv_base).join(python)
    }

    /// Returns the directory where the binaries are stored inside a venv
    pub fn get_venv_bin_dir(&self, venv_base: impl AsRef<Path>) -> PathBuf {
        let venv = venv_base.as_ref();
        if self.is_windows() {
            let bin_dir = venv.join("Scripts");
            if bin_dir.join("python.exe").exists() {
                return bin_dir;
            }
            // Python installed via msys2 on Windows might produce a POSIX-like venv
            // See https://github.com/PyO3/maturin/issues/1108
            let bin_dir = venv.join("bin");
            if bin_dir.join("python.exe").exists() {
                return bin_dir;
            }
            // for conda environment
            venv.to_path_buf()
        } else {
            venv.join("bin")
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
}

fn rustc_version_meta() -> Result<VersionMeta> {
    let meta = rustc_version::version_meta().map_err(|err| match err {
        rustc_version::Error::CouldNotExecuteCommand(e)
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            anyhow!(
                "rustc, the rust compiler, is not installed or not in PATH. \
                     This package requires Rust and Cargo to compile extensions. \
                     Install it through the system's package manager or via https://rustup.rs/.",
            )
        }
        err => anyhow!(err).context("Failed to run rustc to get the host target"),
    })?;
    Ok(meta)
}
