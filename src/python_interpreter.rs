use crate::Manylinux;
use crate::Target;
use failure::{bail, Error, Fail, ResultExt};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str;

/// This snippets will give us information about the python interpreter's
/// version and abi as json through stdout
const GET_INTERPRETER_METADATA: &str = r##"
import sysconfig
import sys
import json

print(json.dumps({
    "major": sys.version_info.major,
    "minor": sys.version_info.minor,
    "abiflags": sysconfig.get_config_var("ABIFLAGS"),
    "m": sysconfig.get_config_var("WITH_PYMALLOC") == 1,
    "u": sysconfig.get_config_var("Py_UNICODE_SIZE") == 4,
    "d": sysconfig.get_config_var("Py_DEBUG") == 1,
    # This one isn't technically necessary, but still very useful for sanity checks
    "platform": sys.platform,
}))
"##;

/// Identifies conditions where we do not want to build wheels
fn windows_interpreter_no_build(
    major: usize,
    minor: usize,
    target_width: usize,
    pointer_width: usize,
) -> bool {
    // Don't use python 2.6
    if major == 2 && minor != 7 {
        return true;
    }

    // Ignore python 3.0 - 3.4
    if major == 3 && minor < 5 {
        return true;
    }

    // There can be 32-bit installations on a 64-bit machine, but we can't link
    // those for 64-bit targets
    if pointer_width != target_width {
        println!(
            "{}.{} is installed as {}-bit, while the target is {}-bit. Skipping.",
            major, minor, pointer_width, target_width
        );
        return true;
    }
    false
}

/// On windows regular Python installs are supported along with environments
/// being managed by `conda`.
///
/// We can't use the linux trick with trying different binary names since on
/// windows the binary is always called "python.exe".  However, whether dealing
/// with regular Python installs or `conda` environments there are tools we can
/// use to query the information regarding installed interpreters.
///
/// Regular Python installs downloaded from Python.org will include the python
/// launcher by default.  We can use the launcher to find the information we need
/// for each installed interpreter using `py -0` which produces something like
/// the following output (the path can by determined using `sys.executable`):
///
/// ```bash
/// Installed Pythons found by py Launcher for Windows
/// -3.7-64 *
/// -3.6-32
/// -2.7-64
/// ```
///
/// When using `conda` we can use the `conda info -e` command to retrieve information
/// regarding the installed interpreters being managed by `conda`.  This is an example
/// of the output expected:
///
/// ```bash
/// # conda environments:
/// #
/// base                     C:\Users\<user-name>\Anaconda3
/// foo1                  *  C:\Users\<user-name>\Anaconda3\envs\foo1
/// foo2                  *  C:\Users\<user-name>\Anaconda3\envs\foo2
/// ```
///
/// The information required can either by obtained by parsing this output directly or
/// by invoking the interpreters to obtain the information.
///
/// As well as the version numbers, etc. of the interpreters we also have to find the
/// pointer width to make sure that the pointer width (32-bit or 64-bit) matches across
/// platforms.
fn find_all_windows(target: &Target) -> Result<Vec<String>, Error> {
    let code = "import sys; print(sys.executable or '')";
    let mut interpreter = vec![];
    let mut versions_found = HashSet::new();

    // If Python is installed from Python.org it should include the "python launcher"
    // which is used to find the installed interpreters
    let execution = Command::new("py").arg("-0").output();
    if let Ok(output) = execution {
        let expr = Regex::new(r" -(\d).(\d)-(\d+)(?: .*)?").unwrap();
        let lines = str::from_utf8(&output.stdout).unwrap().lines();
        for line in lines {
            if let Some(capture) = expr.captures(line) {
                let context = "Expected a digit";

                let major = capture
                    .get(1)
                    .unwrap()
                    .as_str()
                    .parse::<usize>()
                    .context(context)?;
                let minor = capture
                    .get(2)
                    .unwrap()
                    .as_str()
                    .parse::<usize>()
                    .context(context)?;
                if !versions_found.contains(&(major, minor)) {
                    let pointer_width = capture
                        .get(3)
                        .unwrap()
                        .as_str()
                        .parse::<usize>()
                        .context(context)?;

                    if windows_interpreter_no_build(
                        major,
                        minor,
                        target.pointer_width(),
                        pointer_width,
                    ) {
                        continue;
                    }

                    let version = format!("-{}.{}-{}", major, minor, pointer_width);

                    let output = Command::new("py")
                        .args(&[&version, "-c", code])
                        .output()
                        .unwrap();
                    let path = str::from_utf8(&output.stdout).unwrap().trim();
                    if !output.status.success() || path.trim().is_empty() {
                        bail!("Couldn't determine the path to python for `py {}`", version);
                    }
                    interpreter.push(path.to_string());
                    versions_found.insert((major, minor));
                }
            }
        }
    }

    // Conda environments are also supported on windows
    let conda_info = Command::new("conda").arg("info").arg("-e").output();
    if let Ok(output) = conda_info {
        let lines = str::from_utf8(&output.stdout).unwrap().lines();
        // The regex has three parts: The first matches the name and skips
        // comments, the second skips the part in between and the third
        // extracts the path
        let re = Regex::new(r"^([^#].*?)[\s*]+([\w\\:-]+)$").unwrap();
        let mut paths = vec![];
        for i in lines {
            if let Some(capture) = re.captures(&i) {
                if &capture[1] == "base" {
                    continue;
                }
                paths.push(String::from(&capture[2]));
            }
        }

        for path in paths {
            let executable = Path::new(&path).join("python");
            let python_info = Command::new(&executable)
                .arg("-c")
                .arg("import sys; print(sys.version)")
                .output();

            let python_info = match python_info {
                Ok(python_info) => python_info,
                Err(err) => {
                    if err.kind() == io::ErrorKind::NotFound {
                        // This conda env doesn't have python installed
                        continue;
                    } else {
                        bail!(
                            "Error getting Python version info from conda env at {}",
                            path
                        );
                    }
                }
            };

            let version_info = str::from_utf8(&python_info.stdout).unwrap();
            let expr = Regex::new(r"(\d).(\d).(\d+)").unwrap();
            if let Some(capture) = expr.captures(version_info) {
                let major = capture.get(1).unwrap().as_str().parse::<usize>().unwrap();
                let minor = capture.get(2).unwrap().as_str().parse::<usize>().unwrap();
                if !versions_found.contains(&(major, minor)) {
                    let pointer_width = if version_info.contains("64 bit (AMD64)") {
                        64_usize
                    } else {
                        32_usize
                    };

                    if windows_interpreter_no_build(
                        major,
                        minor,
                        target.pointer_width(),
                        pointer_width,
                    ) {
                        continue;
                    }

                    interpreter.push(String::from(executable.to_str().unwrap()));
                    versions_found.insert((major, minor));
                }
            }
        }
    }
    if interpreter.is_empty() {
        bail!(
            "Could not find any interpreters, are you sure you have python installed on your PATH?"
        );
    };
    Ok(interpreter)
}

/// Since there is no known way to list the installed python versions on unix
/// (or just generally to list all binaries in $PATH, which could then be
/// filtered down), this is a workaround (which works until python 4 is
/// released, which won't be too soon)
fn find_all_unix() -> Vec<String> {
    let interpreter = &[
        "python2.7",
        "python3.5",
        "python3.6",
        "python3.7",
        "python3.8",
        "python3.9",
    ];

    interpreter.iter().map(ToString::to_string).collect()
}

/// The output format of [GET_INTERPRETER_METADATA]
#[derive(Serialize, Deserialize)]
struct IntepreterMetadataMessage {
    major: usize,
    minor: usize,
    abiflags: Option<String>,
    m: bool,
    u: bool,
    d: bool,
    platform: String,
}

/// The location and version of an interpreter
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PythonInterpreter {
    /// Python's major version
    pub major: usize,
    /// Python's minor version
    pub minor: usize,
    /// For linux and mac, this contains the value of the abiflags, e.g. "m"
    /// for python3.5m or "mu" for python2.7mu. On windows, the value is
    /// always "".
    ///
    /// See PEP 261 and PEP 393 for details
    pub abiflags: String,
    /// Currently just the value of [Target::os()], i.e. "windows", "linux" or
    /// "macos"
    pub target: Target,
    /// Path to the python interpreter, e.g. /usr/bin/python3.6
    ///
    /// Just the name of the binary in PATH does also work, e.g. `python3.5`
    pub executable: PathBuf,
}

/// Returns the abiflags that are assembled through the message, with some
/// additional sanity checks.
///
/// The rules are as follows:
///  - python 2 + Unix: Assemble the individual parts (m/u/d), no ABIFLAGS
///  - python 2 + Windows: no ABIFLAGS, parts, return an empty string
///  - python 3 + Unix: Use ABIFLAGS
///  - python 3 + Windows: No ABIFLAGS, return an empty string
fn fun_with_abiflags(
    message: &IntepreterMetadataMessage,
    target: &Target,
) -> Result<String, Error> {
    let sane_platform = match message.platform.as_ref() {
        "win32" | "win_amd64" => target.is_windows(),
        "linux" | "linux2" | "linux3" => target.is_linux(),
        "darwin" => target.is_macos(),
        _ => false,
    };

    if !sane_platform {
        bail!(
            "sys.platform in python, {}, and the rust target, {:?}, don't match ಠ_ಠ",
            message.platform,
            target,
        )
    }

    if message.major == 2 {
        let mut abiflags = String::new();
        if message.m {
            abiflags += "m";
        }
        if message.u {
            abiflags += "u";
        }
        if message.d {
            abiflags += "d";
        }

        if message.abiflags.is_some() {
            bail!("A python 2 interpreter does not define abiflags in its sysconfig ಠ_ಠ")
        }

        if abiflags != "" && target.is_windows() {
            bail!("A python 2 interpreter on windows does not define abiflags in its sysconfig ಠ_ಠ")
        }

        Ok(abiflags)
    } else if message.major == 3 && message.minor >= 5 {
        if target.is_windows() {
            if message.abiflags.is_some() {
                bail!("A python 3 interpreter on windows does not define abiflags in its sysconfig ಠ_ಠ")
            } else {
                Ok("".to_string())
            }
        } else if let Some(ref abiflags) = message.abiflags {
            if abiflags != "m" {
                bail!("A python 3 interpreter on linux or mac os must have 'm' as abiflags ಠ_ಠ")
            }
            Ok(abiflags.clone())
        } else {
            bail!("A python 3 interpreter on linux or mac os must define abiflags in its sysconfig ಠ_ಠ")
        }
    } else {
        bail!("Only python 2.7 and python 3.x are supported");
    }
}

impl PythonInterpreter {
    /// Returns the supported python environment in the PEP 425 format:
    /// {python tag}-{abi tag}-{platform tag}
    ///
    /// Don't ask me why or how, this is just what setuptools uses so I'm also going to use
    pub fn get_tag(&self, manylinux: &Manylinux) -> String {
        let platform = self.target.get_platform_tag(manylinux);

        if self.target.is_unix() {
            format!(
                "cp{major}{minor}-cp{major}{minor}{abiflags}-{platform}",
                major = self.major,
                minor = self.minor,
                abiflags = self.abiflags,
                platform = platform
            )
        } else {
            format!(
                "cp{major}{minor}-none-{platform}",
                major = self.major,
                minor = self.minor,
                platform = platform
            )
        }
    }

    /// Generates the correct suffix for shared libraries
    ///
    /// For python 2, it's just `.so`. For python 3, there is PEP 3149, but
    /// that is only valid for 3.2 - 3.4. Since only 3.5+ is supported, the
    /// templates are adapted from the (also
    /// incorrect) release notes of python 3.5:
    /// https://docs.python.org/3/whatsnew/3.5.html#build-and-c-api-changes
    ///
    /// Examples for 64-bit on Python 3.5m:
    /// Linux:   steinlaus.cpython-35m-x86_64-linux-gnu.so
    /// Windows: steinlaus.cp35-win_amd64.pyd
    /// Mac:     steinlaus.cpython-35m-darwin.so
    ///
    /// Examples for 64-bit on Python 2.7mu:
    /// Linux:   steinlaus.so
    /// Windows: steinlaus.pyd
    /// Mac:     steinlaus.so
    pub fn get_library_extension(&self) -> String {
        if self.major == 2 {
            if self.target.is_unix() {
                return ".so".to_string();
            } else {
                return ".pyd".to_string();
            }
        }
        let platform = self.target.get_shared_platform_tag();

        if self.target.is_unix() {
            format!(
                ".cpython-{major}{minor}{abiflags}-{platform}.so",
                major = self.major,
                minor = self.minor,
                abiflags = self.abiflags,
                platform = platform,
            )
        } else {
            format!(
                ".cp{major}{minor}-{platform}.pyd",
                major = self.major,
                minor = self.minor,
                platform = platform
            )
        }
    }

    /// Checks whether the given command is a python interpreter and returns a
    /// [PythonInterpreter] if that is the case
    pub fn check_executable(
        executable: impl AsRef<Path>,
        target: &Target,
    ) -> Result<Option<PythonInterpreter>, Error> {
        let output = Command::new(&executable.as_ref())
            .args(&["-c", GET_INTERPRETER_METADATA])
            .stderr(Stdio::inherit())
            .output();

        let err_msg = format!(
            "Trying to get metadata from the python interpreter {} failed",
            executable.as_ref().display()
        );

        let output = match output {
            Ok(output) => {
                if output.status.success() {
                    output
                } else {
                    bail!(err_msg);
                }
            }
            Err(err) => {
                if err.kind() == io::ErrorKind::NotFound {
                    return Ok(None);
                } else {
                    bail!(err.context(err_msg));
                }
            }
        };
        let message: IntepreterMetadataMessage =
            serde_json::from_slice(&output.stdout).context(err_msg)?;

        if (message.major == 2 && message.minor != 7) || (message.major == 3 && message.minor < 5) {
            return Ok(None);
        }

        let abiflags = fun_with_abiflags(&message, &target)
            .context("Failed to get information from the python interpreter")?;

        Ok(Some(PythonInterpreter {
            major: message.major,
            minor: message.minor,
            abiflags,
            target: target.clone(),
            executable: executable.as_ref().to_path_buf(),
        }))
    }

    /// Tries to find all installed python versions using the heuristic for the
    /// given platform
    pub fn find_all(target: &Target) -> Result<Vec<PythonInterpreter>, Error> {
        let executables = if target.is_windows() {
            find_all_windows(&target)?
        } else {
            find_all_unix()
        };
        let mut available_versions = Vec::new();
        for executable in executables {
            if let Some(version) = PythonInterpreter::check_executable(&executable, &target)? {
                available_versions.push(version);
            }
        }

        Ok(available_versions)
    }

    /// Checks that given list of executables are al valid python intepreters,
    /// determines the abiflags and versions of those interpreters and
    /// returns them as [PythonInterpreter]
    pub fn check_executables(
        executables: &[String],
        target: &Target,
    ) -> Result<Vec<PythonInterpreter>, Error> {
        let mut available_versions = Vec::new();
        for executable in executables {
            if let Some(version) = PythonInterpreter::check_executable(executable, &target)
                .context(format!("{} is not a valid python interpreter", executable))?
            {
                available_versions.push(version);
            } else {
                bail!("{} doesn't exist");
            }
        }

        Ok(available_versions)
    }
}

impl fmt::Display for PythonInterpreter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Python {}.{}{} at {}",
            self.major,
            self.minor,
            self.abiflags,
            self.executable.display()
        )
    }
}
