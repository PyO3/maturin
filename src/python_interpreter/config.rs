use super::{
    InterpreterKind, MAXIMUM_PYPY_MINOR, MAXIMUM_PYTHON_MINOR, MINIMUM_PYPY_MINOR,
    MINIMUM_PYTHON_MINOR,
};
use crate::Target;
use crate::target::{Arch, Os};
use anyhow::{Context, Result, format_err};
use fs_err as fs;
use serde::Deserialize;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::path::Path;

const PYPY_ABI_TAG: &str = "pp73";

fn graalpy_version_for_python_version(major: usize, minor: usize) -> Option<(usize, usize)> {
    match (major, minor) {
        (3, 10) => Some((24, 0)),
        (3, 11) => Some((24, 2)),
        // Since 25.0, GraalPy should only change the major release number for feature releases.
        // Additionally, it promises that only the autumn (oddly-numbered) releases are
        // allowed to break ABI compatibility, so only those can change the Python version.
        // The even-numbered releases will report the ABI version of the previous release.
        // So assuming that GraalPy doesn't fall terribly behind on updating Python version,
        // the version used in the ABI should follow this formula
        (3, 12..) => Some((25 + (minor - 12) * 2, 0)),
        (_, _) => None,
    }
}

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
    /// for python3.7m or "dm" for python3.6dm.
    ///
    /// * Since python3.8, the value is empty
    /// * Since python3.13, the value is "t" for free-threaded builds.
    /// * On Windows, the value was always None.
    ///
    /// See PEP 261 and PEP 393 for details
    pub abiflags: String,
    /// Suffix to use for extension modules as given by sysconfig.
    pub ext_suffix: String,
    /// Pointer width
    pub pointer_width: Option<usize>,
    /// Is GIL disabled, i.e. free-threaded build
    pub gil_disabled: bool,
}

impl InterpreterConfig {
    /// Lookup a wellknown sysconfig for a given Python interpreter
    pub fn lookup_one(
        target: &Target,
        python_impl: InterpreterKind,
        python_version: (usize, usize),
        abiflags: &str,
    ) -> Option<Self> {
        use InterpreterKind::*;

        let (major, minor) = python_version;
        if major < 3 {
            // Python 2 is not supported
            return None;
        }
        let python_ext_arch = target.get_python_ext_arch(python_impl);
        let target_env = target.get_python_target_env(python_impl, python_version);
        let gil_disabled = abiflags == "t";
        match (target.target_os(), python_impl) {
            (Os::Linux, CPython) => {
                let abiflags = if python_version < (3, 8) {
                    "m".to_string()
                } else {
                    abiflags.to_string()
                };
                let ldversion = format!("{major}{minor}{abiflags}");
                let ext_suffix =
                    format!(".cpython-{ldversion}-{python_ext_arch}-linux-{target_env}.so");
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags,
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
                })
            }
            (Os::Linux, PyPy) => {
                let abi_tag = format!("pypy{major}{minor}-{PYPY_ABI_TAG}");
                let ext_suffix = format!(".{abi_tag}-{python_ext_arch}-linux-{target_env}.so");
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: PyPy,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
                })
            }
            (Os::Linux, GraalPy) => {
                let (graalpy_major, graalpy_minor) =
                    graalpy_version_for_python_version(major, minor)?;
                let ext_suffix = format!(
                    ".graalpy{graalpy_major}{graalpy_minor}-{major}{minor}-native-{python_ext_arch}-linux.so"
                );
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: GraalPy,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
                })
            }
            (Os::Macos, CPython) => {
                let abiflags = if python_version < (3, 8) {
                    "m".to_string()
                } else {
                    abiflags.to_string()
                };
                let ldversion = format!("{major}{minor}{abiflags}");
                let ext_suffix = format!(".cpython-{ldversion}-darwin.so");
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags,
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
                })
            }
            (Os::Macos, PyPy) => {
                let ext_suffix = format!(".pypy{major}{minor}-{PYPY_ABI_TAG}-darwin.so");
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: PyPy,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
                })
            }
            (Os::Macos, GraalPy) => {
                let (graalpy_major, graalpy_minor) =
                    graalpy_version_for_python_version(major, minor)?;
                let ext_suffix = format!(
                    ".graalpy{graalpy_major}{graalpy_minor}-{major}{minor}-native-{python_ext_arch}-darwin.so"
                );
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: GraalPy,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
                })
            }
            (Os::Windows, CPython) => {
                let abiflags = if python_version < (3, 8) {
                    "m".to_string()
                } else {
                    abiflags.to_string()
                };
                let ext_suffix = if python_version < (3, 8) {
                    ".pyd".to_string()
                } else {
                    let platform = match target.target_arch() {
                        Arch::Aarch64 => "win_arm64",
                        Arch::X86 => "win32",
                        Arch::X86_64 => "win_amd64",
                        _ => return None,
                    };
                    format!(".cp{major}{minor}{abiflags}-{platform}.pyd")
                };
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags,
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
                })
            }
            (Os::Windows, PyPy) => {
                if target.target_arch() != Arch::X86_64 {
                    // PyPy on Windows only supports x86_64
                    return None;
                }
                let ext_suffix = format!(".pypy{major}{minor}-{PYPY_ABI_TAG}-win_amd64.pyd");
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: PyPy,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
                })
            }
            (Os::FreeBsd, CPython) => {
                let (abiflags, ext_suffix) = if python_version < (3, 8) {
                    ("m".to_string(), ".so".to_string())
                } else {
                    (
                        abiflags.to_string(),
                        format!(".cpython-{major}{minor}{abiflags}.so"),
                    )
                };
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags,
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
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
                    gil_disabled,
                })
            }
            (Os::OpenBsd, CPython) => {
                let ldversion = format!("{major}{minor}");
                let ext_suffix = format!(".cpython-{ldversion}{abiflags}.so");
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
                })
            }
            (Os::Emscripten, CPython) => {
                let ldversion = format!("{major}{minor}");
                let ext_suffix = format!(".cpython-{ldversion}-{python_ext_arch}-emscripten.so");
                Some(Self {
                    major,
                    minor,
                    interpreter_kind: CPython,
                    abiflags: String::new(),
                    ext_suffix,
                    pointer_width: Some(target.pointer_width()),
                    gil_disabled,
                })
            }
            (_, _) => None,
        }
    }

    /// Lookup wellknown sysconfigs for a given target
    pub fn lookup_target(target: &Target) -> Vec<Self> {
        let mut configs = Vec::new();
        for (python_impl, min_minor_ver, max_minor_ver) in [
            (
                InterpreterKind::CPython,
                MINIMUM_PYTHON_MINOR,
                MAXIMUM_PYTHON_MINOR,
            ),
            (
                InterpreterKind::PyPy,
                MINIMUM_PYPY_MINOR,
                MAXIMUM_PYPY_MINOR,
            ),
        ] {
            for minor in min_minor_ver..=max_minor_ver {
                if let Some(config) = Self::lookup_one(target, python_impl, (3, minor), "") {
                    configs.push(config);
                }
            }
            for minor in 13..=max_minor_ver {
                if let Some(config) = Self::lookup_one(target, python_impl, (3, minor), "t") {
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
        let mut build_flags: Option<String> = None;

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
                "build_flags" => parse_value!(build_flags, value),
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
            InterpreterKind::GraalPy => abi_tag.unwrap_or_else(|| {
                let (graalpy_major, graalpy_minor) =
                    graalpy_version_for_python_version(major, minor).unwrap_or((23, 0));
                format!("graalpy{graalpy_major}{graalpy_minor}_{major}{minor}_native")
            }),
        };
        let file_ext = if target.is_windows() { "pyd" } else { "so" };
        let ext_suffix = if target.is_linux() || target.is_macos() || target.is_hurd() {
            let target_env = target.get_python_target_env(interpreter_kind, (major, minor));
            match interpreter_kind {
                InterpreterKind::CPython => ext_suffix.unwrap_or_else(|| {
                    // Eg: .cpython-38-x86_64-linux-gnu.so
                    format!(
                        ".cpython-{}-{}-{}-{}.{}",
                        abi_tag,
                        target.get_python_ext_arch(interpreter_kind),
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
                        target.get_python_ext_arch(interpreter_kind),
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
                        target.get_python_ext_arch(interpreter_kind),
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
                    target.get_python_ext_arch(interpreter_kind),
                    target.get_python_os(),
                    file_ext
                )
            })
        } else {
            ext_suffix.context("missing value for ext_suffix")?
        };
        let gil_disabled = build_flags
            .map(|flags| flags.contains("Py_GIL_DISABLED"))
            .unwrap_or(false);
        Ok(Self {
            major,
            minor,
            interpreter_kind,
            abiflags: abiflags.unwrap_or_default(),
            ext_suffix,
            pointer_width,
            gil_disabled,
        })
    }

    /// Generate pyo3 config file content
    pub fn pyo3_config_file(&self) -> String {
        let build_flags = if self.gil_disabled {
            "Py_GIL_DISABLED"
        } else {
            ""
        };
        let mut content = format!(
            r#"implementation={implementation}
version={major}.{minor}
shared=true
abi3=false
build_flags={build_flags}
suppress_build_script_link_lines=false"#,
            implementation = self.interpreter_kind,
            major = self.major,
            minor = self.minor,
        );
        if let Some(pointer_width) = self.pointer_width {
            write!(content, "\npointer_width={pointer_width}").unwrap();
        }
        if let Ok(lib_dir) = std::env::var("PYO3_CROSS_LIB_DIR") {
            write!(content, "\nlib_dir={}", lib_dir).unwrap();
        }
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_well_known_sysconfigs_linux() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-x86_64-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("i686-unknown-linux-gnu").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-i386-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("aarch64-unknown-linux-gnu").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-aarch64-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("armv7-unknown-linux-gnueabihf").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-arm-linux-gnueabihf.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("arm-unknown-linux-gnueabihf").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-arm-linux-gnueabihf.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("powerpc64le-unknown-linux-gnu").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(
            sysconfig.ext_suffix,
            ".cpython-310-powerpc64le-linux-gnu.so"
        );

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("powerpc-unknown-linux-gnu").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-powerpc-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("mips64-unknown-linux-gnu").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(
            sysconfig.ext_suffix,
            ".cpython-310-mips64-linux-gnuabi64.so"
        );

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("mips-unknown-linux-gnu").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-mips-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("s390x-unknown-linux-gnu").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-s390x-linux-gnu.so");

        // PyPy
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "");
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-x86_64-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("i686-unknown-linux-gnu").unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-x86-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("aarch64-unknown-linux-gnu").unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-aarch64-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("armv7-unknown-linux-gnueabihf").unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-arm-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("arm-unknown-linux-gnueabihf").unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-arm-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("powerpc64le-unknown-linux-gnu").unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-ppc_64-linux-gnu.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("s390x-unknown-linux-gnu").unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-s390x-linux-gnu.so");
    }

    #[test]
    fn test_well_known_sysconfigs_macos() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-apple-darwin").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-darwin.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("aarch64-apple-darwin").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-darwin.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-apple-darwin").unwrap(),
            InterpreterKind::CPython,
            (3, 7),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "m");
        assert_eq!(sysconfig.ext_suffix, ".cpython-37m-darwin.so");

        // PyPy
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-apple-darwin").unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "");
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-darwin.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("aarch64-apple-darwin").unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-darwin.so");
    }

    #[test]
    fn test_well_known_sysconfigs_windows() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-pc-windows-msvc").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cp310-win_amd64.pyd");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("i686-pc-windows-msvc").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cp310-win32.pyd");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("aarch64-pc-windows-msvc").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cp310-win_arm64.pyd");

        // PyPy
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-pc-windows-msvc").unwrap(),
            InterpreterKind::PyPy,
            (3, 9),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".pypy39-pp73-win_amd64.pyd");
    }

    #[test]
    fn test_well_known_sysconfigs_freebsd() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-unknown-freebsd").unwrap(),
            InterpreterKind::CPython,
            (3, 7),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "m");
        assert_eq!(sysconfig.ext_suffix, ".so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-unknown-freebsd").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "");
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("i686-unknown-freebsd").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("aarch64-unknown-freebsd").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("armv7-unknown-freebsd").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");
    }

    #[test]
    fn test_well_known_sysconfigs_netbsd() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-unknown-netbsd").unwrap(),
            InterpreterKind::CPython,
            (3, 7),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "");
        assert_eq!(sysconfig.ext_suffix, ".so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-unknown-netbsd").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".so");
    }

    #[test]
    fn test_well_known_sysconfigs_openbsd() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-unknown-openbsd").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("i686-unknown-openbsd").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");

        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("aarch64-unknown-openbsd").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-310.so");
    }

    #[test]
    fn test_well_known_sysconfigs_emscripten() {
        // CPython
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("wasm32-unknown-emscripten").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.abiflags, "");
        assert_eq!(sysconfig.ext_suffix, ".cpython-310-wasm32-emscripten.so");
    }

    #[test]
    fn test_pyo3_config_file() {
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap(),
            InterpreterKind::CPython,
            (3, 10),
            "",
        )
        .unwrap();
        let config_file = sysconfig.pyo3_config_file();
        let expected = expect![[r#"
            implementation=CPython
            version=3.10
            shared=true
            abi3=false
            build_flags=
            suppress_build_script_link_lines=false
            pointer_width=64"#]];
        expected.assert_eq(&config_file);
    }

    #[test]
    fn test_pyo3_config_file_free_threaded_python_3_13() {
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-unknown-linux-gnu").unwrap(),
            InterpreterKind::CPython,
            (3, 13),
            "t",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-313t-x86_64-linux-gnu.so");
        let config_file = sysconfig.pyo3_config_file();
        let expected = expect![[r#"
            implementation=CPython
            version=3.13
            shared=true
            abi3=false
            build_flags=Py_GIL_DISABLED
            suppress_build_script_link_lines=false
            pointer_width=64"#]];
        expected.assert_eq(&config_file);
    }

    #[test]
    fn test_pyo3_config_file_musl_python_3_11() {
        let sysconfig = InterpreterConfig::lookup_one(
            &Target::from_resolved_target_triple("x86_64-unknown-linux-musl").unwrap(),
            InterpreterKind::CPython,
            (3, 11),
            "",
        )
        .unwrap();
        assert_eq!(sysconfig.ext_suffix, ".cpython-311-x86_64-linux-musl.so");
        let config_file = sysconfig.pyo3_config_file();
        let expected = expect![[r#"
            implementation=CPython
            version=3.11
            shared=true
            abi3=false
            build_flags=
            suppress_build_script_link_lines=false
            pointer_width=64"#]];
        expected.assert_eq(&config_file);
    }
}
