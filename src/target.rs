use crate::cross_compile::is_cross_compiling;
use crate::python_interpreter::InterpreterKind;
use crate::{PlatformTag, PythonInterpreter};
use anyhow::{anyhow, bail, format_err, Context, Result};
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

    /// Returns the platform part of the tag for the wheel name
    pub fn get_platform_tag(
        &self,
        platform_tags: &[PlatformTag],
        universal2: bool,
    ) -> Result<String> {
        let tag = match (&self.os, &self.arch) {
            // Windows
            (Os::Windows, Arch::X86) => "win32".to_string(),
            (Os::Windows, Arch::X86_64) => "win_amd64".to_string(),
            (Os::Windows, Arch::Aarch64) => "win_arm64".to_string(),
            // Linux
            (Os::Linux, _) => {
                let arch = self.get_platform_arch()?;
                let mut platform_tags = platform_tags.to_vec();
                platform_tags.sort();
                let mut tags = vec![];
                for platform_tag in platform_tags {
                    tags.push(format!("{platform_tag}_{arch}"));
                    for alias in platform_tag.aliases() {
                        tags.push(format!("{alias}_{arch}"));
                    }
                }
                tags.join(".")
            }
            // macOS
            (Os::Macos, Arch::X86_64) | (Os::Macos, Arch::Aarch64) => {
                let ((x86_64_major, x86_64_minor), (arm64_major, arm64_minor)) = macosx_deployment_target(env::var("MACOSX_DEPLOYMENT_TARGET").ok().as_deref(), universal2)?;
                if universal2 {
                    format!(
                        "macosx_{x86_64_major}_{x86_64_minor}_x86_64.macosx_{arm64_major}_{arm64_minor}_arm64.macosx_{x86_64_major}_{x86_64_minor}_universal2"
                    )
                } else if self.arch == Arch::Aarch64 {
                    format!("macosx_{arm64_major}_{arm64_minor}_arm64")
                } else {
                    format!("macosx_{x86_64_major}_{x86_64_minor}_x86_64")
                }
            }
            // FreeBSD
            (Os::FreeBsd, _)
            // NetBSD
            | (Os::NetBsd, _)
            // OpenBSD
            | (Os::OpenBsd, _) => {
                let release = self.get_platform_release()?;
                format!(
                    "{}_{}_{}",
                    self.os.to_string().to_ascii_lowercase(),
                    release,
                    self.arch.machine(),
                )
            }
            // DragonFly
            (Os::Dragonfly, Arch::X86_64)
            // Haiku
            | (Os::Haiku, Arch::X86_64) => {
                let release = self.get_platform_release()?;
                format!(
                    "{}_{}_{}",
                    self.os.to_string().to_ascii_lowercase(),
                    release.to_ascii_lowercase(),
                    "x86_64"
                )
            }
            // Emscripten
            (Os::Emscripten, Arch::Wasm32) => {
                let os_version = env::var("MATURIN_EMSCRIPTEN_VERSION");
                let release = match os_version {
                    Ok(os_ver) => os_ver,
                    Err(_) => emcc_version()?,
                };
                let release = release.replace(['.', '-'], "_");
                format!("emscripten_{release}_wasm32")
            }
            (Os::Wasi, Arch::Wasm32) => {
                "any".to_string()
            }
            // osname_release_machine fallback for any POSIX system
            (_, _) => {
                let info = PlatformInfo::new()?;
                let mut release = info.release().replace(['.', '-'], "_");
                let mut machine = info.machine().replace([' ', '/'], "_");

                let mut os = self.os.to_string().to_ascii_lowercase();
                // See https://github.com/python/cpython/blob/46c8d915715aa2bd4d697482aa051fe974d440e1/Lib/sysconfig.py#L722-L730
                if os.starts_with("sunos") {
                    // Solaris / Illumos
                    if let Some((major, other)) = release.split_once('_') {
                        let major_ver: u64 = major.parse().context("illumos major version is not a number")?;
                        if major_ver >= 5 {
                            // SunOS 5 == Solaris 2
                            os = "solaris".to_string();
                            release = format!("{}_{}", major_ver - 3, other);
                            machine = format!("{machine}_64bit");
                        }
                    }
                }
                format!(
                    "{os}_{release}_{machine}"
                )
            }
        };
        Ok(tag)
    }

    fn get_platform_arch(&self) -> Result<String> {
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

    fn get_platform_release(&self) -> Result<String> {
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
    pub fn target_triple(&self) -> &str {
        &self.triple
    }

    /// Returns host triple as string
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
    pub fn target_os(&self) -> Os {
        self.os
    }

    /// Returns target architecture
    pub fn target_arch(&self) -> Arch {
        self.arch
    }

    /// Returns target environment
    pub fn target_env(&self) -> Environment {
        self.env
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

    /// Returns true if the current environment is msvc
    pub fn is_msvc(&self) -> bool {
        self.env == Environment::Msvc
    }

    /// Returns true if the current platform is illumos
    pub fn is_illumos(&self) -> bool {
        self.os == Os::Illumos
    }

    /// Returns true if the current platform is haiku
    pub fn is_haiku(&self) -> bool {
        self.os == Os::Haiku
    }

    /// Returns true if the current platform is Emscripten
    pub fn is_emscripten(&self) -> bool {
        self.os == Os::Emscripten
    }

    /// Returns true if we're building a binary for wasm32-wasi
    pub fn is_wasi(&self) -> bool {
        self.os == Os::Wasi
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
    pub fn get_py3_tags(
        &self,
        platform_tags: &[PlatformTag],
        universal2: bool,
    ) -> Result<Vec<String>> {
        let tags = vec![format!(
            "py3-none-{}",
            self.get_platform_tag(platform_tags, universal2)?
        )];
        Ok(tags)
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
            // Python innstalled via msys2 on Windows might produce a POSIX-like venv
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
        platform_tags: &[PlatformTag],
        universal2: bool,
    ) -> Result<(String, Vec<String>)> {
        let tag = format!(
            "py3-none-{platform}",
            platform = self.get_platform_tag(platform_tags, universal2)?
        );
        let tags = self.get_py3_tags(platform_tags, universal2)?;
        Ok((tag, tags))
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

/// Get the default macOS deployment target version
fn macosx_deployment_target(
    deploy_target: Option<&str>,
    universal2: bool,
) -> Result<((u16, u16), (u16, u16))> {
    let x86_64_default_rustc = rustc_macosx_target_version("x86_64-apple-darwin");
    let x86_64_default = if universal2 && x86_64_default_rustc.1 < 9 {
        (10, 9)
    } else {
        x86_64_default_rustc
    };
    let arm64_default = rustc_macosx_target_version("aarch64-apple-darwin");
    let mut x86_64_ver = x86_64_default;
    let mut arm64_ver = arm64_default;
    if let Some(deploy_target) = deploy_target {
        let err_ctx = "MACOSX_DEPLOYMENT_TARGET is invalid";
        let mut parts = deploy_target.split('.');
        let major = parts.next().context(err_ctx)?;
        let major: u16 = major.parse().context(err_ctx)?;
        let minor = parts.next().context(err_ctx)?;
        let minor: u16 = minor.parse().context(err_ctx)?;
        if (major, minor) > x86_64_default {
            x86_64_ver = (major, minor);
        }
        if (major, minor) > arm64_default {
            arm64_ver = (major, minor);
        }
    }
    Ok((x86_64_ver, arm64_ver))
}

pub(crate) fn rustc_macosx_target_version(target: &str) -> (u16, u16) {
    use std::process::Command;
    use target_lexicon::OperatingSystem;

    let fallback_version = if target == "aarch64-apple-darwin" {
        (11, 0)
    } else {
        (10, 7)
    };

    let rustc_target_version = || -> Result<(u16, u16)> {
        let cmd = Command::new("rustc")
            .arg("-Z")
            .arg("unstable-options")
            .arg("--print")
            .arg("target-spec-json")
            .arg("--target")
            .arg(target)
            .env("RUSTC_BOOTSTRAP", "1")
            .env_remove("MACOSX_DEPLOYMENT_TARGET")
            .output()
            .context("Failed to run rustc to get the target spec")?;
        let stdout = String::from_utf8(cmd.stdout).context("rustc output is not valid utf-8")?;
        let spec: serde_json::Value =
            serde_json::from_str(&stdout).context("rustc output is not valid json")?;
        let llvm_target = spec
            .as_object()
            .context("rustc output is not a json object")?
            .get("llvm-target")
            .context("rustc output does not contain llvm-target")?
            .as_str()
            .context("llvm-target is not a string")?;
        let triple = llvm_target.parse::<Triple>();
        let (major, minor) = match triple.map(|t| t.operating_system) {
            Ok(OperatingSystem::MacOSX { major, minor, .. }) => (major, minor),
            _ => fallback_version,
        };
        Ok((major, minor))
    };
    rustc_target_version().unwrap_or(fallback_version)
}

fn emcc_version() -> Result<String> {
    use regex::bytes::Regex;
    use std::process::Command;

    let emcc = Command::new("emcc")
        .arg("--version")
        .output()
        .context("Failed to run emcc to get the version")?;
    let pattern = Regex::new(r"^emcc .+? (\d+\.\d+\.\d+).*").unwrap();
    let caps = pattern
        .captures(&emcc.stdout)
        .context("Failed to parse emcc version")?;
    let version = caps.get(1).context("Failed to parse emcc version")?;
    Ok(String::from_utf8(version.as_bytes().to_vec())?)
}

#[cfg(test)]
mod test {
    use super::macosx_deployment_target;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_macosx_deployment_target() {
        assert_eq!(
            macosx_deployment_target(None, false).unwrap(),
            ((10, 7), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(None, true).unwrap(),
            ((10, 9), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.6"), false).unwrap(),
            ((10, 7), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.6"), true).unwrap(),
            ((10, 9), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.9"), false).unwrap(),
            ((10, 9), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("11.0.0"), false).unwrap(),
            ((11, 0), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("11.1"), false).unwrap(),
            ((11, 1), (11, 1))
        );
    }
}
