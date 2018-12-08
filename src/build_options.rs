use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use cargo_metadata;
use failure::err_msg;
use failure::{Error, ResultExt};
use toml;

use crate::build_context::BridgeModel;
use crate::cargo_toml::CargoTomlMetadata;
use crate::cargo_toml::CargoTomlMetadataPyo3Pack;
use crate::BuildContext;
use crate::CargoToml;
use crate::Metadata21;
use crate::PythonInterpreter;
use crate::Target;

/// High level API for building wheels from a crate which is also used for the CLI
#[derive(Debug, Serialize, Deserialize, StructOpt, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct BuildOptions {
    #[structopt(short = "i", long = "interpreter")]
    /// The python versions to build wheels for, given as the names of the
    /// interpreters. Uses autodiscovery if not explicitly set.
    pub interpreter: Vec<String>,
    /// Which kind of bindings to use. Possible values are pyo3, rust-cpython, cffi and bin
    #[structopt(short = "b", long = "bindings")]
    pub bindings: Option<String>,
    #[structopt(
        short = "m",
        long = "manifest-path",
        parse(from_os_str),
        default_value = "Cargo.toml",
        name = "PATH"
    )]
    /// The path to the Cargo.toml
    pub manifest_path: PathBuf,
    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    #[structopt(short = "o", long = "out", parse(from_os_str))]
    pub out: Option<PathBuf>,
    /// Don't check for manylinux compliance
    #[structopt(long = "skip-auditwheel")]
    pub skip_auditwheel: bool,
    /// The --target option for cargo
    #[structopt(long = "target", name = "TRIPLE")]
    pub target: Option<String>,
    /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
    #[structopt(long = "cargo-extra-args")]
    pub cargo_extra_args: Vec<String>,
    /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
    #[structopt(long = "rustc-extra-args")]
    pub rustc_extra_args: Vec<String>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            interpreter: vec![],
            bindings: None,
            manifest_path: PathBuf::from("Cargo.toml"),
            out: None,
            skip_auditwheel: false,
            target: None,
            cargo_extra_args: Vec::new(),
            rustc_extra_args: Vec::new(),
        }
    }
}

impl BuildOptions {
    /// Tries to fill the missing metadata in BuildContext by querying cargo and python
    pub fn into_build_context(self, release: bool, strip: bool) -> Result<BuildContext, Error> {
        let manifest_file = self
            .manifest_path
            .canonicalize()
            .map_err(|e| format_err!("Can't find {}: {}", self.manifest_path.display(), e))?;

        if !self.manifest_path.is_file() {
            bail!("{} must be a path to a Cargo.toml", manifest_file.display());
        };
        let contents = fs::read_to_string(&manifest_file).context(format!(
            "Can't read Cargo.toml at {}",
            manifest_file.display(),
        ))?;
        let cargo_toml: CargoToml = toml::from_str(&contents).context(format!(
            "Failed to parse Cargo.toml at {}",
            manifest_file.display()
        ))?;

        let manifest_dir = manifest_file.parent().unwrap().to_path_buf();
        let metadata21 = Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir)
            .context("Failed to parse Cargo.toml into python metadata")?;
        let scripts = match cargo_toml.package.metadata {
            Some(CargoTomlMetadata {
                pyo3_pack:
                    Some(CargoTomlMetadataPyo3Pack {
                        scripts: Some(ref scripts),
                    }),
            }) => scripts.clone(),
            _ => HashMap::new(),
        };

        // If the package name contains minuses, you must declare a module with
        // underscores as lib name
        let module_name = cargo_toml
            .lib
            .clone()
            .and_then(|lib| lib.name)
            .unwrap_or_else(|| cargo_toml.package.name.clone())
            .to_owned();

        let target = Target::from_target_triple(self.target.clone())?;

        // Failure fails here since cargo_metadata does some weird stuff on their side
        let cargo_metadata = cargo_metadata::metadata_deps(Some(&self.manifest_path), true)
            .map_err(|e| format_err!("Cargo metadata failed: {}", e))?;

        let wheel_dir = match self.out {
            Some(ref dir) => dir.clone(),
            None => PathBuf::from(&cargo_metadata.target_directory).join("wheels"),
        };

        let bridge = find_bridge(&cargo_metadata, self.bindings.as_ref().map(|x| &**x))?;

        if bridge != BridgeModel::Bin && module_name.contains('-') {
            bail!(
                "The module name must not contains a minus \
                 (Make sure you have set an appropriate [lib] name in your Cargo.toml)"
            );
        }

        let interpreter = find_interpreter(&bridge, &self.interpreter, &target)?;

        let mut cargo_extra_args = split_extra_args(&self.cargo_extra_args)?;
        if let Some(target) = self.target {
            cargo_extra_args.extend_from_slice(&["--target".to_string(), target]);
        }

        let rustc_extra_args = split_extra_args(&self.rustc_extra_args)?;

        Ok(BuildContext {
            target,
            bridge,
            metadata21,
            scripts,
            module_name,
            manifest_path: self.manifest_path,
            out: wheel_dir,
            release,
            strip,
            skip_auditwheel: self.skip_auditwheel,
            cargo_extra_args,
            rustc_extra_args,
            interpreter,
        })
    }
}

/// Tries to determine the [BridgeModel] for the target crate
pub fn find_bridge(
    cargo_metadata: &cargo_metadata::Metadata,
    bridge: Option<&str>,
) -> Result<BridgeModel, Error> {
    let deps: HashSet<String> = cargo_metadata
        .resolve
        .clone()
        .unwrap()
        .nodes
        .iter()
        .map(|node| node.id.split(' ').nth(0).unwrap().to_string())
        .collect();

    if let Some(bindings) = bridge {
        if bindings == "cffi" {
            Ok(BridgeModel::Cffi)
        } else if bindings == "bin" {
            Ok(BridgeModel::Bin)
        } else {
            if !deps.contains(bindings) {
                bail!(
                    "The bindings crate {} was not found in the dependencies list",
                    bindings
                );
            }

            Ok(BridgeModel::Bindings(bindings.to_string()))
        }
    } else if deps.contains("pyo3") {
        println!("Found pyo3 bindings");
        Ok(BridgeModel::Bindings("pyo3".to_string()))
    } else if deps.contains("cpython") {
        println!("Found rust-python bindings");
        Ok(BridgeModel::Bindings("rust_cpython".to_string()))
    } else {
        bail!("Couldn't find any bindings; Please specify them with -b")
    }
}

/// Finds the appropriate amount for python versions for each [BridgeModel].
///
/// This means all for bindings, one for cffi and zero for bin.
pub fn find_interpreter(
    bridge: &BridgeModel,
    interpreter: &[String],
    target: &Target,
) -> Result<Vec<PythonInterpreter>, Error> {
    Ok(match bridge {
        BridgeModel::Bindings(_) => {
            let interpreter = if !interpreter.is_empty() {
                PythonInterpreter::check_executables(&interpreter, &target)?
            } else {
                PythonInterpreter::find_all(&target)?
            };

            if interpreter.is_empty() {
                bail!("Couldn't find any python interpreters. Please specify at least one with -i");
            }

            println!(
                "Found {}",
                interpreter
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<String>>()
                    .join(", ")
            );

            interpreter
        }
        BridgeModel::Cffi => {
            let executable = if interpreter.is_empty() {
                target.get_python()
            } else if interpreter.len() == 1 {
                PathBuf::from(interpreter[0].clone())
            } else {
                bail!("You can only specify one python interpreter for cffi compilation");
            };
            let err_message = "Failed to find python interpreter for generating cffi bindings";

            let interpreter = PythonInterpreter::check_executable(executable, &target)
                .context(err_msg(err_message))?
                .ok_or_else(|| err_msg(err_message))?;

            println!("Using {} to generate the cffi bindings", interpreter);

            vec![interpreter]
        }
        BridgeModel::Bin => vec![],
    })
}

/// Helper function that calls shlex on all extra args given
fn split_extra_args(given_args: &[String]) -> Result<Vec<String>, Error> {
    let mut splitted_args = vec![];
    for arg in given_args {
        match shlex::split(&arg) {
            Some(split) => splitted_args.extend(split),
            None => {
                bail!(
                    "Couldn't split argument from `--cargo-extra-args`: '{}'",
                    arg
                );
            }
        }
    }
    Ok(splitted_args)
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use super::*;

    #[test]
    fn test_find_bridge_pyo3() {
        let get_fourtytwo = cargo_metadata::metadata_deps(
            Some(&Path::new("get-fourtytwo").join("Cargo.toml")),
            true,
        )
        .unwrap();

        assert!(match find_bridge(&get_fourtytwo, None).unwrap() {
            BridgeModel::Bindings(_) => true,
            _ => false,
        });

        assert!(match find_bridge(&get_fourtytwo, Some("pyo3")).unwrap() {
            BridgeModel::Bindings(_) => true,
            _ => false,
        });

        assert!(find_bridge(&get_fourtytwo, Some("rust-cpython")).is_err());
    }

    #[test]
    fn test_find_bridge_cffi() {
        let points =
            cargo_metadata::metadata_deps(Some(&Path::new("points").join("Cargo.toml")), true)
                .unwrap();

        assert_eq!(
            find_bridge(&points, Some("cffi")).unwrap(),
            BridgeModel::Cffi
        );

        assert!(find_bridge(&points, Some("rust-cpython")).is_err());
        assert!(find_bridge(&points, Some("pyo3")).is_err());
    }

    #[test]
    fn test_find_bridge_bin() {
        let hello_world =
            cargo_metadata::metadata_deps(Some(&Path::new("hello-world").join("Cargo.toml")), true)
                .unwrap();

        assert_eq!(
            find_bridge(&hello_world, Some("bin")).unwrap(),
            BridgeModel::Bin
        );

        assert!(find_bridge(&hello_world, None).is_err());
        assert!(find_bridge(&hello_world, Some("rust-cpython")).is_err());
        assert!(find_bridge(&hello_world, Some("pyo3")).is_err());
    }

    #[test]
    fn test_argument_splitting() {
        let mut options = BuildOptions::default();
        options.cargo_extra_args.push("--features foo".to_string());
        options.bindings = Some("bin".to_string());
        let context = options.into_build_context(false, false).unwrap();
        assert_eq!(context.cargo_extra_args, vec!["--features", "foo"])
    }
}
