use crate::build_context::{BridgeModel, ProjectLayout};
use crate::BuildContext;
use crate::CargoToml;
use crate::Manylinux;
use crate::Metadata21;
use crate::PythonInterpreter;
use crate::Target;
use cargo_metadata::{Metadata, MetadataCommand, Node};
use failure::{bail, err_msg, format_err, Error, ResultExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use structopt::StructOpt;

/// High level API for building wheels from a crate which is also used for the CLI
#[derive(Debug, Serialize, Deserialize, StructOpt, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct BuildOptions {
    // The {n} are workarounds for https://github.com/TeXitoi/structopt/issues/163
    /// Control the platform tag on linux.
    ///
    /// - `1`: Use the manylinux1 tag and check for compliance{n}
    /// - `1-unchecked`: Use the manylinux1 tag without checking for compliance{n}
    /// - `2010`: Use the manylinux2010 tag and check for compliance{n}
    /// - `2010-unchecked`: Use the manylinux1 tag without checking for compliance{n}
    /// - `off`: Use the native linux tag (off)
    ///
    /// This option is ignored on all non-linux platforms
    #[structopt(
        long,
        raw(
            possible_values = r#"&["1", "1-unchecked", "2010", "2010-unchecked", "off"]"#,
            case_insensitive = "true",
            default_value = r#""1""#
        )
    )]
    pub manylinux: Manylinux,
    #[structopt(short, long)]
    /// The python versions to build wheels for, given as the names of the
    /// interpreters. Uses autodiscovery if not explicitly set.
    pub interpreter: Vec<String>,
    /// Which kind of bindings to use. Possible values are pyo3, rust-cpython, cffi and bin
    #[structopt(short, long)]
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
    #[structopt(short, long, parse(from_os_str))]
    pub out: Option<PathBuf>,
    /// [deprecated, use --manylinux instead] Don't check for manylinux compliance
    #[structopt(long = "skip-auditwheel")]
    pub skip_auditwheel: bool,
    /// The --target option for cargo
    #[structopt(long, name = "TRIPLE")]
    pub target: Option<String>,
    /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
    ///
    /// Use as `--cargo-extra-args="--my-arg"`
    #[structopt(long = "cargo-extra-args")]
    pub cargo_extra_args: Vec<String>,
    /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
    ///
    /// Use as `--rustc-extra-args="--my-arg"`
    #[structopt(long = "rustc-extra-args")]
    pub rustc_extra_args: Vec<String>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            manylinux: Manylinux::Manylinux1,
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
    /// Tries to fill the missing metadata for a BuildContext by querying cargo and python
    pub fn into_build_context(self, release: bool, strip: bool) -> Result<BuildContext, Error> {
        let manifest_file = self
            .manifest_path
            .canonicalize()
            .context(format_err!("Can't find {}", self.manifest_path.display()))?;

        if !manifest_file.is_file() {
            bail!(
                "{} (resolved to {}) is not the path to a Cargo.toml",
                self.manifest_path.display(),
                manifest_file.display()
            );
        };

        let cargo_toml = CargoToml::from_path(&manifest_file)?;
        let manifest_dir = manifest_file.parent().unwrap();
        let metadata21 = Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir)
            .context("Failed to parse Cargo.toml into python metadata")?;
        let scripts = cargo_toml.scripts();

        // If the package name contains minuses, you must declare a module with
        // underscores as lib name
        let module_name = cargo_toml
            .lib
            .clone()
            .and_then(|lib| lib.name)
            .unwrap_or_else(|| cargo_toml.package.name.clone())
            .to_owned();

        let project_layout = ProjectLayout::determine(manifest_dir, &module_name)?;

        let target = Target::from_target_triple(self.target.clone())?;

        // Failure fails here since cargo_metadata does some weird stuff on their side
        let cargo_metadata = MetadataCommand::new()
            .manifest_path(&self.manifest_path)
            .exec()
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

        let manylinux = if self.skip_auditwheel {
            eprintln!("⚠ --skip-auditwheel is deprecated, use --manylinux=1-unchecked");
            Manylinux::Manylinux1Unchecked
        } else {
            self.manylinux
        };

        Ok(BuildContext {
            target,
            bridge,
            project_layout,
            metadata21,
            scripts,
            module_name,
            manifest_path: self.manifest_path,
            out: wheel_dir,
            release,
            strip,
            manylinux,
            cargo_extra_args,
            rustc_extra_args,
            interpreter,
            cargo_metadata,
        })
    }
}

/// Tries to determine the [BridgeModel] for the target crate
pub fn find_bridge(cargo_metadata: &Metadata, bridge: Option<&str>) -> Result<BridgeModel, Error> {
    let nodes = cargo_metadata
        .resolve
        .clone()
        .ok_or_else(|| format_err!("Expected to get a dependency graph from cargo"))?
        .nodes;
    let deps: HashMap<String, Node> = nodes
        .iter()
        .map(|node| (cargo_metadata[&node.id].name.clone(), node.clone()))
        .collect();

    if let Some(bindings) = bridge {
        if bindings == "cffi" {
            Ok(BridgeModel::Cffi)
        } else if bindings == "bin" {
            Ok(BridgeModel::Bin)
        } else {
            if !deps.contains_key(bindings) {
                bail!(
                    "The bindings crate {} was not found in the dependencies list",
                    bindings
                );
            }

            Ok(BridgeModel::Bindings(bindings.to_string()))
        }
    } else if let Some(node) = deps.get("pyo3") {
        println!("🔗 Found pyo3 bindings");
        if !node.features.contains(&"extension-module".to_string()) {
            let version = cargo_metadata[&node.id].version.to_string();
            println!(
                "⚠  Warning: You're building a library without activating pyo3's \
                 `extension-module` feature. \
                 See https://pyo3.rs/{}/building-and-distribution.html#linking",
                version
            );
        }
        Ok(BridgeModel::Bindings("pyo3".to_string()))
    } else if deps.contains_key("cpython") {
        println!("🔗 Found rust-cpython bindings");
        Ok(BridgeModel::Bindings("rust_cpython".to_string()))
    } else {
        bail!("Couldn't find any bindings; Please specify them with --bindings/-b")
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
                PythonInterpreter::check_executables(&interpreter, &target)
                    .context("The given list of python interpreters is invalid")?
            } else {
                PythonInterpreter::find_all(&target)
                    .context("Finding python interpreters failed")?
            };

            if interpreter.is_empty() {
                bail!("Couldn't find any python interpreters. Please specify at least one with -i");
            }

            println!(
                "🐍 Found {}",
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

            println!("🐍 Using {} to generate the cffi bindings", interpreter);

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
        let pyo3_pure = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/pyo3-pure").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(match find_bridge(&pyo3_pure, None).unwrap() {
            BridgeModel::Bindings(_) => true,
            _ => false,
        });

        assert!(match find_bridge(&pyo3_pure, Some("pyo3")).unwrap() {
            BridgeModel::Bindings(_) => true,
            _ => false,
        });

        assert!(find_bridge(&pyo3_pure, Some("rust-cpython")).is_err());
    }

    #[test]
    fn test_find_bridge_cffi() {
        let cffi_pure = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/cffi-pure").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert_eq!(
            find_bridge(&cffi_pure, Some("cffi")).unwrap(),
            BridgeModel::Cffi
        );

        assert!(find_bridge(&cffi_pure, Some("rust-cpython")).is_err());
        assert!(find_bridge(&cffi_pure, Some("pyo3")).is_err());
    }

    #[test]
    fn test_find_bridge_bin() {
        let hello_world = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/hello-world").join("Cargo.toml"))
            .exec()
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
