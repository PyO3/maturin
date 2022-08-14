use crate::{PythonInterpreter, Target};
use anyhow::{bail, Result};
use fs_err::{self as fs, DirEntry};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

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

    if let Some(target_without_env) = target_triple
        .rfind('-')
        .map(|index| &target_triple[0..index])
    {
        if host.starts_with(target_without_env) {
            // Not cross-compiling if arch-vendor-os is all the same
            // e.g. x86_64-unknown-linux-musl on x86_64-unknown-linux-gnu host
            return Ok(false);
        }
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
    let sysconfig_paths = search_lib_dir(lib_dir, target);
    let sysconfig_name = env::var_os("_PYTHON_SYSCONFIGDATA_NAME");
    let mut sysconfig_paths = sysconfig_paths
        .iter()
        .filter_map(|p| {
            let canonical = fs::canonicalize(p).ok();
            match &sysconfig_name {
                Some(_) => canonical.filter(|p| p.file_stem() == sysconfig_name.as_deref()),
                None => canonical,
            }
        })
        .collect::<Vec<PathBuf>>();
    sysconfig_paths.dedup();
    if sysconfig_paths.is_empty() {
        bail!(
            "Could not find either libpython.so or _sysconfigdata*.py in {}",
            lib_dir.display()
        );
    } else if sysconfig_paths.len() > 1 {
        bail!(
            "Detected multiple possible python versions, please set the PYO3_PYTHON_VERSION \
            variable to the wanted version on your system or set the _PYTHON_SYSCONFIGDATA_NAME \
            variable to the wanted sysconfigdata file name\nsysconfigdata paths = {:?}",
            sysconfig_paths
        )
    }

    Ok(sysconfig_paths.remove(0))
}

/// recursive search for _sysconfigdata, returns all possibilities of sysconfigdata paths
fn search_lib_dir(path: impl AsRef<Path>, target: &Target) -> Vec<PathBuf> {
    let mut sysconfig_paths = vec![];
    let (cpython_version_pat, pypy_version_pat) = if let Some(v) =
        env::var_os("PYO3_CROSS_PYTHON_VERSION").map(|s| s.into_string().unwrap())
    {
        (format!("python{}", v), format!("pypy{}", v))
    } else {
        ("python3.".into(), "pypy3.".into())
    };
    for f in fs::read_dir(path.as_ref()).expect("Path does not exist") {
        let sysc = match &f {
            Ok(f) if starts_with(f, "_sysconfigdata") && ends_with(f, "py") => vec![f.path()],
            Ok(f) if starts_with(f, "build") => search_lib_dir(f.path(), target),
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
                search_lib_dir(f.path(), target)
            }
            Ok(f) if starts_with(f, &cpython_version_pat) => search_lib_dir(f.path(), target),
            // PyPy 3.7: /opt/python/pp37-pypy37_pp73/lib_pypy/_sysconfigdata__linux_x86_64-linux-gnu.py
            Ok(f) if starts_with(f, "lib_pypy") => search_lib_dir(f.path(), target),
            // PyPy 3.8: /opt/python/pp38-pypy38_pp73/lib/pypy3.8/_sysconfigdata__linux_x86_64-linux-gnu.py
            Ok(f) if starts_with(f, &pypy_version_pat) => search_lib_dir(f.path(), target),
            Ok(f) if starts_with(f, "lib") && f.path().is_dir() => search_lib_dir(f.path(), target),
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
    sysconfig_paths
}
