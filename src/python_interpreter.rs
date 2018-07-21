use failure::{Error, Fail, ResultExt};
use serde_json;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use target_info::Target;

/// The location and version of an interpreter
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct PythonInterpreter {
    /// Python's major version
    pub major: usize,
    /// Python's minor version
    pub minor: usize,
    /// The abi; There are e.g. python2.7m and python2.7mu; Other abis are not supported.
    ///
    /// See PEP 261 and PEP 393 for details
    pub has_u: bool,
    /// Path to the python interpreter, e.g. /usr/bin/python3.6
    ///
    /// Just the name of the binary in PATH does also work, e.g. `python3.5`
    pub executable: PathBuf,
}

impl PythonInterpreter {
    /// Returns the supported python environment in the PEP 425 format:
    /// {python tag}-{abi tag}-{platform tag}
    pub fn get_tag(&self) -> String {
        // Don't ask me why, this is just what setuptools uses so I'm also going to use it
        let platform = match Target::os() {
            "linux" => "manylinux1_x86_64",
            "macos" => {
                "macosx_10_6_intel.\
                 macosx_10_9_intel.\
                 macosx_10_9_x86_64.\
                 macosx_10_10_intel.\
                 macosx_10_10_x86_64"
            }
            "windows" => if Target::pointer_width() == "64" {
                "win_amd64"
            } else {
                "win32"
            },
            _ => panic!("This platform is not supported"),
        };
        format!(
            "cp{major}{minor}-cp{major}{minor}{abiflags}-{platform}",
            major = self.major,
            minor = self.minor,
            abiflags = if self.has_u { "mu" } else { "m" },
            platform = platform
        )
    }

    /// Checks which python version of a set of possible versions are avaible and determins whether
    /// they are m or mu
    pub fn find_all(python_versions: &[String]) -> Result<Vec<PythonInterpreter>, Error> {
        let mut available_versions = Vec::new();
        let import = "import json, sysconfig;";
        let format = "value = {k:str(v) for k, v in sysconfig.get_config_vars().items()};";
        let dump = r#"print(json.dumps(value))"#;
        for executable in python_versions {
            let output = Command::new(&executable)
                .args(&["-c", &format!(r#"{}{}{}"#, import, format, dump)])
                .stderr(Stdio::inherit())
                .output();

            let err_msg = format!("Trying to get the sysconfig from {} failed", executable);

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
                        continue;
                    } else {
                        bail!(err.context(err_msg));
                    }
                }
            };

            let values: HashMap<String, String> =
                serde_json::from_slice(&output.stdout).context(err_msg)?;

            let version = values
                .get("VERSION")
                .ok_or_else(|| format_err!("sysconfig does not define a version"))?;
            let split: Vec<_> = version.split('.').collect();
            let major = split[0].parse::<usize>()?;
            let minor = split[1].parse::<usize>()?;

            if major == 3 && minor < 5 {
                bail!("Only python 2.7 and python 3.5 are supported");
            }

            let has_u;
            if let Some(abiflags) = values.get("ABIFLAGS") {
                match abiflags.as_ref() {
                    "m" => has_u = false,
                    "mu" => has_u = true,
                    _ => bail!(
                        r#"Only the "m" and "mu" abiflags are suppoted, not "{}""#,
                        abiflags
                    ),
                };
            } else if let Some(unicode_size) = values.get("Py_UNICODE_SIZE") {
                match unicode_size.as_ref() {
                    "2" => has_u = false,
                    "4" => has_u = true,
                    _ => bail!(
                        "Invalid unicode size: {} (Expected either 2 or 4)",
                        unicode_size
                    ),
                };
            } else {
                bail!(
                    "Expected the sysconfig module of {} to define a value for abiflags",
                    executable
                );
            }

            available_versions.push(PythonInterpreter {
                major,
                minor,
                has_u,
                executable: PathBuf::from(executable),
            });
        }

        Ok(available_versions)
    }
}

impl fmt::Display for PythonInterpreter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Python {}.{}{} at {}",
            self.major,
            self.minor,
            if self.has_u { "mu" } else { "m" },
            self.executable.display()
        )
    }
}
