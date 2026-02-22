pub use self::config::InterpreterConfig;
use crate::auditwheel::PlatformTag;
use crate::{BuildContext, Target};
use anyhow::Result;
use std::fmt;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;

mod abiflags;
mod config;
mod discovery;
mod resolver;

pub(crate) use self::resolver::{InterpreterResolver, ResolveResult};

pub const MINIMUM_PYTHON_MINOR: usize = 7;
pub const MINIMUM_PYPY_MINOR: usize = 8;
/// Be liberal here to include preview versions
pub const MAXIMUM_PYTHON_MINOR: usize = 14;
pub const MAXIMUM_PYPY_MINOR: usize = 11;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lower")]
pub enum InterpreterKind {
    CPython,
    PyPy,
    GraalPy,
}

impl InterpreterKind {
    /// Is this a CPython interpreter?
    pub fn is_cpython(&self) -> bool {
        matches!(self, InterpreterKind::CPython)
    }

    /// Is this a PyPy interpreter?
    pub fn is_pypy(&self) -> bool {
        matches!(self, InterpreterKind::PyPy)
    }

    /// Is this a GraalPy interpreter?
    pub fn is_graalpy(&self) -> bool {
        matches!(self, InterpreterKind::GraalPy)
    }
}

impl fmt::Display for InterpreterKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            InterpreterKind::CPython => write!(f, "CPython"),
            InterpreterKind::PyPy => write!(f, "PyPy"),
            InterpreterKind::GraalPy => write!(f, "GraalVM"),
        }
    }
}

impl FromStr for InterpreterKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "cpython" => Ok(InterpreterKind::CPython),
            "pypy" => Ok(InterpreterKind::PyPy),
            "graalvm" | "graalpy" => Ok(InterpreterKind::GraalPy),
            unknown => Err(format!("Unknown interpreter kind '{unknown}'")),
        }
    }
}

// ---------------------------------------------------------------------------
// PythonInterpreter
// ---------------------------------------------------------------------------

/// The location and version of an interpreter
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PythonInterpreter {
    /// Python's sysconfig
    /// Python's major version
    pub config: InterpreterConfig,
    /// Path to the python interpreter, e.g. /usr/bin/python3.6
    ///
    /// Just the name of the binary in PATH does also work, e.g. `python3.5`
    pub executable: PathBuf,
    /// Comes from `sysconfig.get_platform()`
    ///
    /// Note that this can be `None` when cross compiling
    pub platform: Option<String>,
    /// Is this interpreter runnable
    ///
    /// When cross compile the target interpreter isn't runnable,
    /// and it's `executable` is empty
    pub runnable: bool,
    /// Comes from `sys.platform.name`
    pub implementation_name: String,
    /// Comes from sysconfig var `SOABI`
    pub soabi: Option<String>,
}

impl Deref for PythonInterpreter {
    type Target = InterpreterConfig;

    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

impl PythonInterpreter {
    /// Does this interpreter have PEP 384 stable api aka. abi3 support?
    pub fn has_stable_api(&self) -> bool {
        if self.implementation_name.parse::<InterpreterKind>().is_err() {
            false
        } else {
            match self.interpreter_kind {
                // Free-threaded python does not have stable api support yet
                InterpreterKind::CPython => !self.config.gil_disabled,
                InterpreterKind::PyPy | InterpreterKind::GraalPy => false,
            }
        }
    }

    /// Returns the supported python environment in the PEP 425 format used for the wheel filename:
    /// {python tag}-{abi tag}-{platform tag}
    ///
    /// Don't ask me why or how, this is just what setuptools uses so I'm also going to use
    ///
    /// If abi3 is true, cpython wheels use the generic abi3 with the given version as minimum
    pub fn get_tag(&self, context: &BuildContext, platform_tags: &[PlatformTag]) -> Result<String> {
        // Restrict `sysconfig.get_platform()` usage to Windows and non-portable Linux only for now
        // so we don't need to deal with macOS deployment target
        let target = &context.target;
        let use_sysconfig_platform = target.is_windows()
            || (target.is_linux() && platform_tags.iter().any(|tag| !tag.is_portable()))
            || target.is_illumos();
        let platform = if use_sysconfig_platform {
            if let Some(platform) = self.platform.clone() {
                platform
            } else {
                context.get_platform_tag(platform_tags)?
            }
        } else {
            context.get_platform_tag(platform_tags)?
        };
        let tag = if self.implementation_name.parse::<InterpreterKind>().is_err() {
            // Use generic tags when `sys.implementation.name` != `platform.python_implementation()`, for example Pyston
            // See also https://github.com/pypa/packaging/blob/0031046f7fad649580bc3127d1cef9157da0dd79/packaging/tags.py#L234-L261
            format!(
                "{interpreter}{major}{minor}-{soabi}-{platform}",
                interpreter = self.implementation_name,
                major = self.major,
                minor = self.minor,
                soabi = self
                    .soabi
                    .as_deref()
                    .unwrap_or("none")
                    .replace(['-', '.'], "_"),
                platform = platform
            )
        } else {
            match self.interpreter_kind {
                InterpreterKind::CPython => {
                    format!(
                        "cp{major}{minor}-cp{major}{minor}{abiflags}-{platform}",
                        major = self.major,
                        minor = self.minor,
                        abiflags = self.abiflags,
                        platform = platform
                    )
                }
                InterpreterKind::PyPy => {
                    // pypy uses its version as part of the ABI, e.g.
                    // pypy 3.11 7.3 => numpy-1.20.1-pp311-pypy311_pp73-manylinux2014_x86_64.whl
                    format!(
                        "pp{major}{minor}-{abi_tag}-{platform}",
                        major = self.major,
                        minor = self.minor,
                        abi_tag = abiflags::calculate_abi_tag(&self.ext_suffix)
                            .expect("PyPy's syconfig didn't define a valid `EXT_SUFFIX` ಠ_ಠ"),
                        platform = platform,
                    )
                }
                InterpreterKind::GraalPy => {
                    // GraalPy like PyPy uses its version as part of the ABI
                    // graalpy 3.10 23.1 => numpy-1.23.5-graalpy310-graalpy231_310_native-manylinux2014_x86_64.whl
                    format!(
                        "graalpy{major}{minor}-{abi_tag}-{platform}",
                        major = self.major,
                        minor = self.minor,
                        abi_tag = abiflags::calculate_abi_tag(&self.ext_suffix)
                            .expect("GraalPy's syconfig didn't define a valid `EXT_SUFFIX` ಠ_ಠ"),
                        platform = platform,
                    )
                }
            }
        };
        Ok(tag)
    }

    /// Adds the ext_suffix we read from python or know (.pyd/.abi3.so) and adds it to the base name
    ///
    /// For CPython, generate extensions as follows:
    ///
    /// For python 3, there is PEP 3149, but
    /// that is only valid for 3.2 - 3.4. Since only 3.6+ is supported, the
    /// templates are adapted from the (also
    /// incorrect) release notes of CPython 3.5:
    /// https://docs.python.org/3/whatsnew/3.5.html#build-and-c-api-changes
    ///
    /// Examples for 64-bit on CPython 3.6m:
    /// Linux:   foobar.cpython-36m-x86_64-linux-gnu.so
    /// Windows: foobar.cp36-win_amd64.pyd
    /// Mac:     foobar.cpython-36m-darwin.so
    /// FreeBSD: foobar.cpython-36m.so
    ///
    /// For pypy3, we read importlib.machinery.EXTENSION_SUFFIXES[0].
    pub fn get_library_name(&self, base: &str) -> String {
        format!(
            "{base}{ext_suffix}",
            base = base,
            ext_suffix = self.ext_suffix
        )
    }

    /// Is this a debug build of Python for Windows?
    pub fn is_windows_debug(&self) -> bool {
        self.ext_suffix.starts_with("_d.") && self.ext_suffix.ends_with(".pyd")
    }

    /// Construct a `PythonInterpreter` from a sysconfig and target
    pub fn from_config(config: InterpreterConfig) -> Self {
        let implementation_name = config.interpreter_kind.to_string().to_ascii_lowercase();
        PythonInterpreter {
            config,
            executable: PathBuf::new(),
            platform: None,
            runnable: false,
            implementation_name,
            soabi: None,
        }
    }

    /// Whether this Python interpreter support portable manylinux/musllinux wheels
    ///
    /// Returns `true` if we can not decide
    pub fn support_portable_wheels(&self) -> bool {
        if !self.runnable {
            return true;
        }
        let out = std::process::Command::new(&self.executable)
            .args([
                "-m",
                "pip",
                "debug",
                "--verbose",
                "--disable-pip-version-check",
            ])
            .output();

        match out {
            Err(_) => true,
            Ok(ok) if !ok.status.success() => true,
            Ok(ok) => {
                if let Ok(stdout) = String::from_utf8(ok.stdout) {
                    stdout.contains("manylinux") || stdout.contains("musllinux")
                } else {
                    true
                }
            }
        }
    }

    /// An opaque string that uniquely identifies this Python interpreter.
    /// Used to trigger rebuilds for `pyo3` when the Python interpreter changes.
    pub fn environment_signature(&self) -> String {
        let pointer_width = self.pointer_width.unwrap_or(64);
        format!(
            "{}-{}.{}-{}bit",
            self.implementation_name, self.major, self.minor, pointer_width
        )
    }

    /// Returns the site-packages directory inside a venv e.g.
    /// {venv_base}/lib/python{x}.{y} on unix or {venv_base}/Lib on window
    pub fn get_venv_site_package(&self, venv_base: impl AsRef<Path>, target: &Target) -> PathBuf {
        if target.is_unix() {
            match self.interpreter_kind {
                InterpreterKind::CPython | InterpreterKind::GraalPy => {
                    let python_dir = format!("python{}.{}", self.major, self.minor);
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
}

impl fmt::Display for PythonInterpreter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.runnable {
            write!(
                f,
                "{} {}.{}{} at {}",
                self.config.interpreter_kind,
                self.config.major,
                self.config.minor,
                self.config.abiflags,
                self.executable.display()
            )
        } else {
            write!(
                f,
                "{} {}.{}{}",
                self.config.interpreter_kind,
                self.config.major,
                self.config.minor,
                self.config.abiflags,
            )
        }
    }
}
