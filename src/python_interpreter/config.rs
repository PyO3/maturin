use super::{InterpreterKind, MAXIMUM_PYPY_MINOR, MAXIMUM_PYTHON_MINOR, MINIMUM_PYTHON_MINOR};
use crate::target::{Arch, Os};
use crate::Target;
use anyhow::{format_err, Context, Result};
use fs_err as fs;
use serde::Deserialize;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::path::Path;

const PYPY_ABI_TAG: &str = "pp73";
const GRAALPY_ABI_TAG: &str = "graalpy230_310_native";

/// Some of the sysconfigdata of Python interpreter we care about
#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
pub struct InterpreterConfig {
    /// Python's major version
    pub major: usize,
    /// Python's minor version
    pub minor: usize,
    /// cpython, pypy, or graalpy
    #[serde(rename = "interpreter")]
    pub interpreter_kind: InterpreterKind,
    /// For linux and mac, this contains the value of the abiflags, e.g. "m"
    /// for python3.7m or "dm" for python3.6dm. Since python3.8, the value is
    /// empty. On windows, the value was always None.
    ///
    /// See PEP 261 and PEP 393 for details
    pub abiflags: String,
    /// Suffix to use for extension modules as given by sysconfig.
    pub ext_suffix: String,
    /// Pointer width
    pub pointer_width: Option<usize>,
}

impl InterpreterConfig {
    /// Lookup a wellknown sysconfig for a given Python interpreter
    pub fn lookup_one(
        target: &Target,
        python_impl: InterpreterKind,
        python_version: (usize, usize),
    ) -> Option<Self> {
        use InterpreterKind::*;

        let (major, minor) = python_version;
        if major < 3 {
            // Python 2 is not supported
            return None;
        }
        let python_arch = if matches!(target.target_arch(), Arch::Armv6L | Arch::Armv7L) {
            "arm"
        } else if matches!(target.target_arch(), Arch::Powerpc64Le) && python_impl == PyPy {
            "ppc_64"
        } else if matches!(target.target_arch(), Arch::X86) && python_impl == PyPy {
            "x86"
        } else {
            target.get_python_arch()
        };
        // See https://github.com/pypa/auditwheel/issues/349
        let target_env = match python_impl {
            CPython => {
                if python_version >= (3, 11) {
                    target.target_env().to_string()
                } else {
                    target.target_env().to_string().replace("musl", "gnu")
                }
            }
            PyPy | GraalPy => "gnu".to_string(),
        };
        match (target.target_os(), python_impl) {
            (Os::Linux, CPython) => {
                let abiflags = if python_version < (3, 8) {
                    "m".to_string()
                } else {
                    "".to_string()
                };
                let ldversion = format!("{}{}{}", major, minor, abiflags);
                let ext_suffix = format!(
                    ".cpython-{}-{}-linux-{}.so",
                    ldversion, python_arch, target_env
                );
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags,
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                })
            }
            (Os::Linux, PyPy) => {
                let abi_tag = format!("pypy{}{}-{}", major, minor, PYPY_ABI_TAG);
                let ext_suffix = format!(".{}-{}-linux-{}.so", abi_tag, python_arch, target_env);
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: PyPy,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                })
            }
            (Os::Macos, CPython) => {
                let abiflags = if python_version < (3, 8) {
                    "m".to_string()
                } else {
                    "".to_string()
                };
                let ldversion = format!("{}{}{}", major, minor, abiflags);
                let ext_suffix = format!(".cpython-{}-darwin.so", ldversion);
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags,
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                })
            }
            (Os::Macos, PyPy) => {
                let ext_suffix = format!(".pypy{}{}-{}-darwin.so", major, minor, PYPY_ABI_TAG);
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: PyPy,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                })
            }
            (Os::Windows, CPython) => {
                let ext_suffix = if python_version < (3, 8) {
                    ".pyd".to_string()
                } else {
                    let platform = match target.target_arch() {
                        Arch::Aarch64 => "win_arm64",
                        Arch::X86 => "win32",
                        Arch::X86_64 => "win_amd64",
                        _ => return None,
                    };
                    format!(".cp{}{}-{}.pyd", major, minor, platform)
                };
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                })
            }
            (Os::Windows, PyPy) => {
                if target.target_arch() != Arch::X86_64 {
                    // PyPy on Windows only supports x86_64
                    return None;
                }
                let ext_suffix = format!(".pypy{}{}-{}-win_amd64.pyd", major, minor, PYPY_ABI_TAG);
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: PyPy,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                })
            }
            (Os::FreeBsd, CPython) => {
                let (abiflags, ext_suffix) = if python_version < (3, 8) {
                    ("m".to_string(), ".so".to_string())
                } else {
                    ("".to_string(), format!(".cpython-{}{}.so", major, minor))
                };
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags,
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                })
            }
            (Os::NetBsd, CPython) => {
                let ext_suffix = ".so".to_string();
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                })
            }
            (Os::OpenBsd, CPython) => {
                let ldversion = format!("{}{}", major, minor);
                let ext_suffix = format!(".cpython-{}.so", ldversion);
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                })
            }
            (Os::Emscripten, CPython) => {
                let ldversion = format!("{}{}", major, minor);
                let ext_suffix = format!(".cpython-{}-{}-emscripten.so", ldversion, python_arch);
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                })
            }
            (_, _) => None,
        }
    }

    /// Lookup wellknown sysconfigs for a given target
    pub fn lookup_target(target: &Target) -> Vec<Self> {
        let mut configs = Vec::new();
        for (python_impl, max_minor_ver) in [
            (InterpreterKind::CPython, MAXIMUM_PYTHON_MINOR),
            (InterpreterKind::PyPy, MAXIMUM_PYPY_MINOR),
        ] {
            for minor in MINIMUM_PYTHON_MINOR..=max_minor_ver {
                if let Some(config) = Self::lookup_one(target, python_impl, (3, minor)) {
                    configs.push(config);
                }
            }
        }
        configs
    }

    /// Construct a new InterpreterConfig from a pyo3 config file
    pub fn from_pyo3_config(config_file: &Path, target: &Target) -> Result<Self> {
        let config_file = fs::File::open(config_file)?;
        let reader = BufReader::new(config_file);
        let lines = reader.lines();

        macro_rules! parse_value {
            ($variable:ident, $value:ident) => {
                $variable = Some($value.trim().parse().context(format!(
                    concat!(
                        "failed to parse ",
                        stringify!($variable),
                        " from config value '{}'"
                    ),
                    $value
                ))?)
            };
        }

        let mut implementation = None;
        let mut version = None;
        let mut abiflags = None;
        let mut ext_suffix = None;
        let mut abi_tag = None;
        let mut pointer_width = None;

        for (i, line) in lines.enumerate() {
            let line = line.context("failed to read line from config")?;
            let (key, value) = line
                .split_once('=')
                .with_context(|| format!("expected key=value pair on line {}", i + 1))?;
            match key {
                "implementation" => parse_value!(implementation, value),
                "version" => parse_value!(version, value),
                "abiflags" => parse_value!(abiflags, value),
                "ext_suffix" => parse_value!(ext_suffix, value),
                "abi_tag" => parse_value!(abi_tag, value),
                "pointer_width" => parse_value!(pointer_width, value),
                _ => continue,
            }
        }
        let version: String = version.context("missing value for version")?;
        let (ver_major, ver_minor) = version
            .split_once('.')
            .context("Invalid python interpreter version")?;
        let major = ver_major.parse::<usize>().with_context(|| {
            format!("Invalid python interpreter major version '{ver_major}', expect a digit")
        })?;
        let minor = ver_minor.parse::<usize>().with_context(|| {
            format!("Invalid python interpreter minor version '{ver_minor}', expect a digit")
        })?;
        let implementation = implementation.unwrap_or_else(|| "cpython".to_string());
        let interpreter_kind = implementation.parse().map_err(|e| format_err!("{}", e))?;
        let abi_tag = match interpreter_kind {
            InterpreterKind::CPython => {
                if (major, minor) >= (3, 8) {
                    abi_tag.unwrap_or_else(|| format!("{major}{minor}"))
                } else {
                    abi_tag.unwrap_or_else(|| format!("{major}{minor}m"))
                }
            }
            InterpreterKind::PyPy => abi_tag.unwrap_or_else(|| PYPY_ABI_TAG.to_string()),
            InterpreterKind::GraalPy => abi_tag.unwrap_or_else(|| GRAALPY_ABI_TAG.to_string()),
        };
        let file_ext = if target.is_windows() { "pyd" } else { "so" };
        let ext_suffix = if target.is_linux() || target.is_macos() {
            // See https://github.com/pypa/auditwheel/issues/349
            let target_env = if (major, minor) >= (3, 11) {
                target.target_env().to_string()
            } else {
                target.target_env().to_string().replace("musl", "gnu")
            };
            match interpreter_kind {
                InterpreterKind::CPython => ext_suffix.unwrap_or_else(|| {
                    // Eg: .cpython-38-x86_64-linux-gnu.so
                    format!(
                        ".cpython-{}-{}-{}-{}.{}",
                        abi_tag,
                        target.get_python_arch(),
                        target.get_python_os(),
                        target_env,
                        file_ext,
                    )
                }),
                InterpreterKind::PyPy => ext_suffix.unwrap_or_else(|| {
                    // Eg: .pypy38-pp73-x86_64-linux-gnu.so
                    format!(
                        ".pypy{}{}-{}-{}-{}-{}.{}",
                        major,
                        minor,
                        abi_tag,
                        target.get_python_arch(),
                        target.get_python_os(),
                        target_env,
                        file_ext,
                    )
                }),
                InterpreterKind::GraalPy => ext_suffix.unwrap_or_else(|| {
                    // e.g. .graalpy230-310-native-x86_64-linux.so
                    format!(
                        ".{}-{}-{}.{}",
                        abi_tag.replace('_', "-"),
                        target.get_python_arch(),
                        target.get_python_os(),
                        file_ext,
                    )
                }),
            }
        } else if target.is_emscripten() && matches!(interpreter_kind, InterpreterKind::CPython) {
            ext_suffix.unwrap_or_else(|| {
                format!(
                    ".cpython-{}-{}-{}.{}",
                    abi_tag,
                    target.get_python_arch(),
                    target.get_python_os(),
                    file_ext
                )
            })
        } else {
            ext_suffix.context("missing value for ext_suffix")?
        };
        Ok(Self {
            major,
            minor,
            interpreter_kind,
            abiflags: abiflags.unwrap_or_default(),
            ext_suffix,
            pointer_width,
        })
    }

    /// Generate pyo3 config file content
    pub fn pyo3_config_file(&self) -> String {
        let mut content = format!(
            r#"implementation={implementation}
version={major}.{minor}
shared=true
abi3=false
build_flags=WITH_THREAD
suppress_build_script_link_lines=false"#,
            implementation = self.interpreter_kind,
            major = self.major,
            minor = self.minor,
        );
        if let Some(pointer_width) = self.pointer_width {
            write!(content, "\npointer_width={pointer_width}").unwrap();
        }
        content
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use expect_test::expect;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_well_known_sysconfigs_linux() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-x86_64-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("i686-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-i386-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("aarch64-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-aarch64-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("armv7-unknown-linux-gnueabihf".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-arm-linux-gnueabihf.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("arm-unknown-linux-gnueabihf".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-arm-linux-gnueabihf.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("powerpc64le-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(
            sysconfig.ext_suffix,
            ".cpython-310-powerpc64le-linux-gnu.so"
        );

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("s390x-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-s390x-linux-gnu.so");

        // PyPy
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "");
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-x86_64-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("i686-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-x86-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("aarch64-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-aarch64-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("armv7-unknown-linux-gnueabihf".to_string())).unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-arm-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("arm-unknown-linux-gnueabihf".to_string())).unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-arm-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("powerpc64le-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-ppc_64-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("s390x-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-s390x-linux-gnu.so");
    }

    #[test]
    fn test_well_known_sysconfigs_macos() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-apple-darwin".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-darwin.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("aarch64-apple-darwin".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-darwin.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-apple-darwin".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 7),
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "m");
        assert_eq!(sysconfig.ext_suffix, ".cpython-37m-darwin.so");

        // PyPy
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-apple-darwin".to_string())).unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "");
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-darwin.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("aarch64-apple-darwin".to_string())).unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-darwin.so");
    }

    #[test]
    fn test_well_known_sysconfigs_windows() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-pc-windows-msvc".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cp310-win_amd64.pyd");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("i686-pc-windows-msvc".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cp310-win32.pyd");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("aarch64-pc-windows-msvc".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cp310-win_arm64.pyd");

        // PyPy
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-pc-windows-msvc".to_string())).unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-win_amd64.pyd");
    }

    #[test]
    fn test_well_known_sysconfigs_freebsd() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-unknown-freebsd".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 7),
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "m");
        assert_eq!(sysconfig.ext_suffix, ".so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-unknown-freebsd".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "");
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("i686-unknown-freebsd".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("aarch64-unknown-freebsd".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("armv7-unknown-freebsd".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");
    }

    #[test]
    fn test_well_known_sysconfigs_netbsd() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-unknown-netbsd".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 7),
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "");
        assert_eq!(sysconfig.ext_suffix, ".so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-unknown-netbsd".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".so");
    }

    #[test]
    fn test_well_known_sysconfigs_openbsd() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-unknown-openbsd".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("i686-unknown-openbsd".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("aarch64-unknown-openbsd".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");
    }

    #[test]
    fn test_well_known_sysconfigs_emscripten() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("wasm32-unknown-emscripten".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "");
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-wasm32-emscripten.so");
    }

    #[test]
    fn test_pyo3_config_file() {
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-unknown-linux-gnu".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 10),
        )
        .unwrap();
        let config_file = sysconfig.pyo3_config_file();
        let expected = expect![[r#"
            implementation=CPython
            version=3.10
            shared=true
            abi3=false
            build_flags=WITH_THREAD
            suppress_build_script_link_lines=false
            pointer_width=64"#]];
        expected.assert_eq(&config_file);
    }

    #[test]
    fn test_pyo3_config_file_musl_python_3_11() {
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_target_triple(Some("x86_64-unknown-linux-musl".to_string())).unwrap(),
            InterpreterKind::CPython,
            (3, 11),
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-311-x86_64-linux-musl.so");
        let config_file = sysconfig.pyo3_config_file();
        let expected = expect![[r#"
            implementation=CPython
            version=3.11
            shared=true
            abi3=false
            build_flags=WITH_THREAD
            suppress_build_script_link_lines=false
            pointer_width=64"#]];
        expected.assert_eq(&config_file);
    }
}
