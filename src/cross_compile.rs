use crate::target::get_host_target;
use crate::Target;
use anyhow::{bail, Result};
use fs_err::{self as fs, DirEntry};
use std::collections::HashMap;
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn is_cross_compiling(target: &Target) -> Result<bool> {
    let target_triple = target.target_triple();
    let host = get_host_target()?;
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
    interpreter: &Path,
    config_path: impl AsRef<Path>,
) -> Result<HashMap<String, String>> {
    let mut script = fs::read_to_string(config_path)?;
    script += r#"
print("version_major", build_time_vars["VERSION"][0])  # 3
print("version_minor", build_time_vars["VERSION"][2])  # E.g., 8
KEYS = [
    "ABIFLAGS",
    "EXT_SUFFIX",
    "SOABI",
]
for key in KEYS:
    print(key, build_time_vars.get(key, ""))
"#;
    let output = run_python_script(interpreter, &script)?;

    Ok(parse_script_output(&output))
}

fn parse_script_output(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let mut i = line.splitn(2, ' ');
            Some((i.next()?.into(), i.next()?.into()))
        })
        .collect()
}

/// Run a python script using the specified interpreter binary.
fn run_python_script(interpreter: &Path, script: &str) -> Result<String> {
    let out = Command::new(interpreter)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(script.as_bytes())?;
            child.wait_with_output()
        });

    match out {
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound {
                bail!(
                    "Could not find any interpreter at {}, \
                     are you sure you have Python installed on your PATH?",
                    interpreter.display()
                );
            } else {
                bail!(
                    "Failed to run the Python interpreter at {}: {}",
                    interpreter.display(),
                    err
                );
            }
        }
        Ok(ok) if !ok.status.success() => bail!("Python script failed"),
        Ok(ok) => Ok(String::from_utf8(ok.stdout)?),
    }
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
/// Where get_platform returns a kebab-case formated string containing the os, the architecture and
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
pub fn find_sysconfigdata(lib_dir: &Path) -> Result<PathBuf> {
    let sysconfig_paths = search_lib_dir(lib_dir);
    let mut sysconfig_paths = sysconfig_paths
        .iter()
        .filter_map(|p| fs::canonicalize(p).ok())
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
            variable to the wanted version on your system\nsysconfigdata paths = {:?}",
            sysconfig_paths
        )
    }

    Ok(sysconfig_paths.remove(0))
}

/// recursive search for _sysconfigdata, returns all possibilities of sysconfigdata paths
fn search_lib_dir(path: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut sysconfig_paths = vec![];
    let version_pat = if let Some(v) =
        env::var_os("PYO3_CROSS_PYTHON_VERSION").map(|s| s.into_string().unwrap())
    {
        format!("python{}", v)
    } else {
        "python3.".into()
    };
    for f in fs::read_dir(path.as_ref()).expect("Path does not exist") {
        let sysc = match &f {
            Ok(f) if starts_with(f, "_sysconfigdata") && ends_with(f, "py") => vec![f.path()],
            Ok(f) if starts_with(f, "build") => search_lib_dir(f.path()),
            Ok(f) if starts_with(f, "lib.") => {
                let name = f.file_name();
                // check if right target os
                let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
                if !name
                    .to_string_lossy()
                    .contains(if os == "android" { "linux" } else { &os })
                {
                    continue;
                }
                // Check if right arch
                if !name
                    .to_string_lossy()
                    .contains(&env::var("CARGO_CFG_TARGET_ARCH").unwrap())
                {
                    continue;
                }
                search_lib_dir(f.path())
            }
            Ok(f) if starts_with(f, &version_pat) => search_lib_dir(f.path()),
            _ => continue,
        };
        sysconfig_paths.extend(sysc);
    }
    sysconfig_paths
}
