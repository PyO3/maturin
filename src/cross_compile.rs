use crate::python_interpreter::{InterpreterConfig, InterpreterKind};
use crate::target::Os;
use crate::{PythonInterpreter, Target};
use anyhow::{Context, Result, bail};
use fs_err::{self as fs, DirEntry};
use normpath::PathExt as _;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use tracing::debug;

pub fn is_cross_compiling(target: &Target) -> Result<bool> {
    let target_triple = target.target_triple();
    let host = target.host_triple();
    if target_triple == host {
        // Not cross-compiling
        return Ok(false);
    }

    if target_triple == "x86_64-apple-darwin" && host == "aarch64-apple-darwin" {
        // Not cross-compiling to compile for x86-64 Python from macOS arm64
        return Ok(false);
    }
    if target_triple == "aarch64-apple-darwin" && host == "x86_64-apple-darwin" {
        // Not cross-compiling to compile for arm64 Python from macOS x86_64
        return Ok(false);
    }

    if target_triple.starts_with("i686-pc-windows") && host.starts_with("x86_64-pc-windows") {
        // Not cross-compiling to compile for 32-bit Python from windows 64-bit
        return Ok(false);
    }
    if target_triple.starts_with("x86_64-pc-windows") && host.starts_with("aarch64-pc-windows") {
        // Not cross-compiling to compile for x86-64 Python from Windows arm64,
        // Windows arm64 can run x86-64 binaries natively
        return Ok(false);
    }
    if target_triple.ends_with("windows-gnu") && host.ends_with("windows-msvc") {
        // Not cross-compiling to compile for Windows GNU from Windows MSVC host
        return Ok(false);
    }

    if target.target_os() == Os::Ios {
        // Not cross-compiling to compile for iOS. There's no on-device compilation,
        // so compilation will always be in a "fake" cross-platform venv with a
        // working python/sysconfig that can interrogated.
        return Ok(false);
    }

    Ok(true)
}

/// Parse sysconfigdata file
///
/// The sysconfigdata is simply a dictionary containing all the build time variables used for the
/// python executable and library. Here it is read and added to a script to extract only what is
/// necessary. This necessitates a python interpreter for the host machine to work.
pub fn parse_sysconfigdata(
    interpreter: &PythonInterpreter,
    config_path: impl AsRef<Path>,
) -> Result<HashMap<String, String>> {
    let mut script = fs::read_to_string(config_path)?;
    script += r#"
print("version_major", build_time_vars["VERSION"][0])  # 3
print("version_minor", build_time_vars["VERSION"][2:])  # E.g., 8, 10
KEYS = [
    "ABIFLAGS",
    "EXT_SUFFIX",
    "SOABI",
    "Py_GIL_DISABLED",
]
for key in KEYS:
    print(key, build_time_vars.get(key, ""))
"#;
    let output = interpreter.run_script(&script)?;

    Ok(parse_script_output(&output))
}

fn parse_script_output(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            line.split_once(' ')
                .map(|(x, y)| (x.to_string(), y.to_string()))
        })
        .collect()
}

fn starts_with(entry: &DirEntry, pat: &str) -> bool {
    let name = entry.file_name();
    name.to_string_lossy().starts_with(pat)
}
fn ends_with(entry: &DirEntry, pat: &str) -> bool {
    let name = entry.file_name();
    name.to_string_lossy().ends_with(pat)
}

/// Finds the `_sysconfigdata*.py` file in the library path.
///
/// From the python source for `_sysconfigdata*.py` is always going to be located at
/// `build/lib.{PLATFORM}-{PY_MINOR_VERSION}` when built from source. The [exact line][1] is defined as:
///
/// ```py
/// pybuilddir = 'build/lib.%s-%s' % (get_platform(), sys.version_info[:2])
/// ```
///
/// Where get_platform returns a kebab-case formatted string containing the os, the architecture and
/// possibly the os' kernel version (not the case on linux). However, when installed using a package
/// manager, the `_sysconfigdata*.py` file is installed in the `${PREFIX}/lib/python3.Y/` directory.
/// The `_sysconfigdata*.py` is generally in a sub-directory of the location of `libpython3.Y.so`.
/// So we must find the file in the following possible locations:
///
/// ```sh
/// # distribution from package manager, lib_dir should include lib/
/// ${INSTALL_PREFIX}/lib/python3.Y/_sysconfigdata*.py
/// ${INSTALL_PREFIX}/lib/libpython3.Y.so
/// ${INSTALL_PREFIX}/lib/python3.Y/config-3.Y-${HOST_TRIPLE}/libpython3.Y.so
///
/// # Built from source from host
/// ${CROSS_COMPILED_LOCATION}/build/lib.linux-x86_64-Y/_sysconfigdata*.py
/// ${CROSS_COMPILED_LOCATION}/libpython3.Y.so
///
/// # if cross compiled, kernel release is only present on certain OS targets.
/// ${CROSS_COMPILED_LOCATION}/build/lib.{OS}(-{OS-KERNEL-RELEASE})?-{ARCH}-Y/_sysconfigdata*.py
/// ${CROSS_COMPILED_LOCATION}/libpython3.Y.so
/// ```
///
/// [1]: https://github.com/python/cpython/blob/3.5/Lib/sysconfig.py#L389
pub fn find_sysconfigdata(lib_dir: &Path, target: &Target) -> Result<PathBuf> {
    let sysconfig_paths = search_lib_dir(lib_dir, target)?;
    let sysconfig_name = env::var_os("_PYTHON_SYSCONFIGDATA_NAME");
    let mut sysconfig_paths = sysconfig_paths
        .iter()
        .filter_map(|p| {
            let canonical = p.normalize().ok().map(|p| p.into_path_buf());
            match &sysconfig_name {
                Some(_) => canonical.filter(|p| p.file_stem() == sysconfig_name.as_deref()),
                None => canonical,
            }
        })
        .collect::<Vec<PathBuf>>();
    sysconfig_paths.dedup();
    if sysconfig_paths.is_empty() {
        bail!("Could not find _sysconfigdata*.py in {}", lib_dir.display());
    } else if sysconfig_paths.len() > 1 {
        bail!(
            "Detected multiple possible python versions, please set the PYO3_CROSS_PYTHON_VERSION \
            variable to the wanted version on your system or set the _PYTHON_SYSCONFIGDATA_NAME \
            variable to the wanted sysconfigdata file name\nsysconfigdata paths = {:?}",
            sysconfig_paths
        )
    }

    Ok(sysconfig_paths.remove(0))
}

/// recursive search for _sysconfigdata, returns all possibilities of sysconfigdata paths
fn search_lib_dir(path: impl AsRef<Path>, target: &Target) -> Result<Vec<PathBuf>> {
    let mut sysconfig_paths = vec![];
    let (cpython_version_pat, pypy_version_pat) = if let Some(v) =
        env::var_os("PYO3_CROSS_PYTHON_VERSION").map(|s| s.into_string().unwrap())
    {
        (format!("python{v}"), format!("pypy{v}"))
    } else {
        ("python3.".into(), "pypy3.".into())
    };
    for f in fs::read_dir(path.as_ref())? {
        let sysc = match &f {
            Ok(f) if starts_with(f, "_sysconfigdata") && ends_with(f, "py") => vec![f.path()],
            Ok(f) if starts_with(f, "build") && f.path().is_dir() => {
                search_lib_dir(f.path(), target)?
            }
            Ok(f) if starts_with(f, "lib.") => {
                let name = f.file_name();
                // check if right target os
                if !name.to_string_lossy().contains(target.get_python_os()) {
                    continue;
                }
                // Check if right arch
                if !name
                    .to_string_lossy()
                    .contains(&target.target_arch().to_string())
                {
                    continue;
                }
                search_lib_dir(f.path(), target)?
            }
            Ok(f) if starts_with(f, &cpython_version_pat) => search_lib_dir(f.path(), target)?,
            // PyPy 3.7: /opt/python/pp37-pypy37_pp73/lib_pypy/_sysconfigdata__linux_x86_64-linux-gnu.py
            Ok(f) if starts_with(f, "lib_pypy") => search_lib_dir(f.path(), target)?,
            // PyPy 3.8: /opt/python/pp38-pypy38_pp73/lib/pypy3.8/_sysconfigdata__linux_x86_64-linux-gnu.py
            Ok(f) if starts_with(f, &pypy_version_pat) => search_lib_dir(f.path(), target)?,
            Ok(f) if starts_with(f, "lib") && f.path().is_dir() => {
                search_lib_dir(f.path(), target)?
            }
            _ => continue,
        };
        sysconfig_paths.extend(sysc);
    }
    // If we got more than one file, only take those that contain the arch name.
    // For ubuntu 20.04 with host architecture x86_64 and a foreign architecture of armhf
    // this reduces the number of candidates to 1:
    //
    // $ find /usr/lib/python3.8/ -name '_sysconfigdata*.py' -not -lname '*'
    //  /usr/lib/python3.8/_sysconfigdata__x86_64-linux-gnu.py
    //  /usr/lib/python3.8/_sysconfigdata__arm-linux-gnueabihf.py
    if sysconfig_paths.len() > 1 {
        let temp = sysconfig_paths
            .iter()
            .filter(|p| {
                p.to_string_lossy()
                    .contains(&target.target_arch().to_string())
            })
            .cloned()
            .collect::<Vec<PathBuf>>();
        if !temp.is_empty() {
            sysconfig_paths = temp;
        }
    }
    Ok(sysconfig_paths)
}

/// PEP 739 `build-details.json` schema types
#[derive(Deserialize)]
struct BuildDetails {
    language: BuildDetailsLanguage,
    implementation: BuildDetailsImplementation,
    abi: Option<BuildDetailsAbi>,
}

#[derive(Deserialize)]
struct BuildDetailsLanguage {
    version: String,
}

#[derive(Deserialize)]
struct BuildDetailsImplementation {
    name: String,
}

#[derive(Deserialize)]
struct BuildDetailsAbi {
    flags: Option<Vec<String>>,
    extension_suffix: String,
}

/// Search for `build-details.json` in the given lib directory.
///
/// Starting from Python 3.14, the file is installed in the platform-independent
/// standard library directory, e.g. `<prefix>/lib/python3.14/build-details.json`.
pub fn find_build_details(path: &Path) -> Option<PathBuf> {
    let candidate = path.join("build-details.json");
    if candidate.is_file() {
        return Some(candidate);
    }
    let cross_python_version =
        env::var_os("PYO3_CROSS_PYTHON_VERSION").map(|s| s.into_string().unwrap());
    let version_pat = cross_python_version
        .as_deref()
        .map(|v| format!("python{v}"))
        .unwrap_or_else(|| "python3.".into());
    let entries = fs::read_dir(path).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let dominated = name_str.starts_with(&version_pat)
            || name_str.starts_with("lib")
            || name_str.ends_with(".framework")
            || name_str == "Frameworks"
            || name_str == "Versions"
            || name_str.starts_with("3.");
        if dominated
            && entry.path().is_dir()
            && let Some(found) = find_build_details(&entry.path())
        {
            return Some(found);
        }
    }
    None
}

/// Read and parse a PEP 739 `build-details.json` file into an `InterpreterConfig`.
pub fn parse_build_details_json_file(path: &Path) -> Result<InterpreterConfig> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    parse_build_details(&content).with_context(|| format!("Failed to parse {}", path.display()))
}

/// Parse PEP 739 `build-details.json` content into an `InterpreterConfig`.
///
/// This allows cross-compilation without needing a host Python interpreter,
/// since the file is static JSON that can be read directly.
pub fn parse_build_details(content: &str) -> Result<InterpreterConfig> {
    let details: BuildDetails =
        serde_json::from_str(content).context("Invalid build-details.json")?;

    let (major, minor) = details
        .language
        .version
        .split_once('.')
        .context("Invalid language.version in build-details.json")?;
    let major = major
        .parse::<usize>()
        .context("Invalid major version in build-details.json")?;
    let minor = minor
        .parse::<usize>()
        .context("Invalid minor version in build-details.json")?;

    let impl_name = details.implementation.name.to_ascii_lowercase();
    let interpreter_kind = match impl_name.as_str() {
        "cpython" => InterpreterKind::CPython,
        "pypy" => InterpreterKind::PyPy,
        "graalpy" => InterpreterKind::GraalPy,
        other => bail!("Unsupported Python implementation in build-details.json: {other}"),
    };

    let abi = details.abi.context(
        "build-details.json is missing the 'abi' section, cannot determine extension suffix",
    )?;

    let abiflags = abi.flags.as_deref().unwrap_or_default().join("");
    let gil_disabled = abi
        .flags
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|f| f == "t");
    let ext_suffix = abi.extension_suffix.clone();

    debug!(
        "Parsed build-details.json: {interpreter_kind} {major}.{minor}, ext_suffix={ext_suffix}, abiflags={abiflags}"
    );

    Ok(InterpreterConfig {
        major,
        minor,
        interpreter_kind,
        abiflags,
        ext_suffix,
        pointer_width: None,
        gil_disabled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_build_details_cpython() {
        let config = parse_build_details(
            r#"{
                "schema_version": "1.0",
                "base_prefix": "/usr",
                "platform": "linux-x86_64",
                "language": { "version": "3.14" },
                "implementation": {
                    "name": "cpython",
                    "version": { "major": 3, "minor": 14, "micro": 0, "releaselevel": "final", "serial": 0 },
                    "hexversion": 51249312,
                    "cache_tag": "cpython-314"
                },
                "abi": {
                    "flags": [],
                    "extension_suffix": ".cpython-314-x86_64-linux-gnu.so",
                    "stable_abi_suffix": ".abi3.so"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(config.major, 3);
        assert_eq!(config.minor, 14);
        assert_eq!(config.interpreter_kind, InterpreterKind::CPython);
        assert_eq!(config.abiflags, "");
        assert_eq!(config.ext_suffix, ".cpython-314-x86_64-linux-gnu.so");
        assert!(!config.gil_disabled);
    }

    #[test]
    fn test_parse_build_details_free_threaded() {
        let config = parse_build_details(
            r#"{
                "schema_version": "1.0",
                "base_prefix": "/usr",
                "platform": "linux-x86_64",
                "language": { "version": "3.14" },
                "implementation": {
                    "name": "cpython",
                    "version": { "major": 3, "minor": 14, "micro": 0, "releaselevel": "final", "serial": 0 },
                    "hexversion": 51249312,
                    "cache_tag": "cpython-314"
                },
                "abi": {
                    "flags": ["t"],
                    "extension_suffix": ".cpython-314t-x86_64-linux-gnu.so"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(config.abiflags, "t");
        assert_eq!(config.ext_suffix, ".cpython-314t-x86_64-linux-gnu.so");
        assert!(config.gil_disabled);
    }

    #[test]
    fn test_parse_build_details_debug_free_threaded() {
        let config = parse_build_details(
            r#"{
                "schema_version": "1.0",
                "base_prefix": "/usr",
                "platform": "linux-x86_64",
                "language": { "version": "3.14" },
                "implementation": {
                    "name": "cpython",
                    "version": { "major": 3, "minor": 14, "micro": 0, "releaselevel": "alpha", "serial": 0 },
                    "hexversion": 51249312,
                    "cache_tag": "cpython-314"
                },
                "abi": {
                    "flags": ["t", "d"],
                    "extension_suffix": ".cpython-314td-x86_64-linux-gnu.so"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(config.abiflags, "td");
        assert!(config.gil_disabled);
    }

    #[test]
    fn test_parse_build_details_missing_abi() {
        let result = parse_build_details(
            r#"{
                "schema_version": "1.0",
                "base_prefix": "/usr",
                "platform": "linux-x86_64",
                "language": { "version": "3.14" },
                "implementation": {
                    "name": "cpython",
                    "version": { "major": 3, "minor": 14, "micro": 0, "releaselevel": "final", "serial": 0 },
                    "hexversion": 51249312,
                    "cache_tag": "cpython-314"
                }
            }"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_find_build_details_direct() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("build-details.json");
        fs::write(&path, r#"{"schema_version":"1.0"}"#).unwrap();

        let found = find_build_details(dir.path());
        assert_eq!(found, Some(path));
    }

    #[test]
    fn test_find_build_details_in_python_subdir() {
        let dir = TempDir::new().unwrap();
        let pydir = dir.path().join("lib").join("python3.14");
        fs::create_dir_all(&pydir).unwrap();
        let path = pydir.join("build-details.json");
        fs::write(&path, r#"{"schema_version":"1.0"}"#).unwrap();

        let found = find_build_details(dir.path());
        assert_eq!(found, Some(path));
    }

    #[test]
    fn test_find_build_details_in_framework_layout() {
        let dir = TempDir::new().unwrap();
        let pydir = dir
            .path()
            .join("Frameworks")
            .join("Python.framework")
            .join("Versions")
            .join("3.14")
            .join("lib")
            .join("python3.14");
        fs::create_dir_all(&pydir).unwrap();
        let path = pydir.join("build-details.json");
        fs::write(&path, r#"{"schema_version":"1.0"}"#).unwrap();

        let found = find_build_details(dir.path());
        assert_eq!(found, Some(path));
    }

    #[test]
    fn test_find_build_details_not_present() {
        let dir = TempDir::new().unwrap();
        let found = find_build_details(dir.path());
        assert!(found.is_none());
    }
}
