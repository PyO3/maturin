use build_context::BridgeModel;
use cargo_metadata;
use cargo_toml::CargoTomlMetadata;
use cargo_toml::CargoTomlMetadataPyo3Pack;
use failure::{Error, ResultExt};
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use toml;
use BuildContext;
use CargoToml;
use Metadata21;
use PythonInterpreter;
use Target;

/// High level API for building wheels from a crate which can be also used for
/// the CLI
#[derive(Debug, Serialize, Deserialize, StructOpt, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct BuildOptions {
    #[structopt(short = "i", long = "interpreter")]
    /// The python versions to build wheels for, given as the names of the
    /// interpreters. Uses autodiscovery if not explicitly set.
    pub interpreter: Vec<String>,
    /// The crate providing the python bindings. pyo3, rust-cpython and cffi are supported
    #[structopt(short = "b", long = "bindings-crate")]
    pub bindings: Option<String>,
    #[structopt(
        short = "m",
        long = "manifest-path",
        parse(from_os_str),
        default_value = "Cargo.toml"
    )]
    /// The path to the Cargo.toml
    pub manifest_path: PathBuf,
    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    #[structopt(short = "o", long = "out", parse(from_os_str))]
    pub out: Option<PathBuf>,
    /// Do a debug build (don't pass --release to cargo)
    #[structopt(short = "d", long = "debug")]
    pub debug: bool,
    /// Don't check for manylinux compliance
    #[structopt(long = "skip-auditwheel")]
    pub skip_auditwheel: bool,
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
            debug: false,
            skip_auditwheel: false,
            cargo_extra_args: Vec::new(),
            rustc_extra_args: Vec::new(),
        }
    }
}

impl BuildOptions {
    /// Tries to fill the missing metadata in BuildContext by querying cargo and python
    pub fn into_build_context(self) -> Result<BuildContext, Error> {
        let manifest_file = self.manifest_path.canonicalize().unwrap();

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
            .name
            .clone()
            .unwrap_or_else(|| cargo_toml.package.name.clone())
            .to_owned();

        if module_name.contains('-') {
            bail!(
                "The module name must not contains a minus \
                 (Make sure you have set an appropriate [lib] name in your Cargo.toml)"
            );
        }

        let target = Target::current();

        // Failure fails here since cargo_metadata does some weird stuff on their side
        let cargo_metadata = cargo_metadata::metadata_deps(Some(&self.manifest_path), true)
            .map_err(|e| format_err!("Cargo metadata failed: {}", e))?;

        let wheel_dir = match self.out {
            Some(ref dir) => dir.clone(),
            None => PathBuf::from(&cargo_metadata.target_directory).join("wheels"),
        };

        let bridge = find_bridge(&cargo_metadata, &self.bindings, &self.interpreter, &target)?;

        Ok(BuildContext {
            target,
            bridge,
            metadata21,
            scripts,
            module_name,
            manifest_path: self.manifest_path,
            out: wheel_dir,
            debug: self.debug,
            skip_auditwheel: self.skip_auditwheel,
            cargo_extra_args: self.cargo_extra_args,
            rustc_extra_args: self.rustc_extra_args,
        })
    }
}

pub fn find_bridge(
    cargo_metadata: &cargo_metadata::Metadata,
    bridge: &Option<String>,
    executables: &[String],
    target: &Target,
) -> Result<BridgeModel, Error> {
    let deps: HashSet<String> = cargo_metadata
        .resolve
        .clone()
        .unwrap()
        .nodes
        .iter()
        .map(|node| node.id.split(' ').nth(0).unwrap().to_string())
        .collect();

    let bridge_crate = if let Some(ref bindings) = bridge {
        if bindings == "cffi" {
            None
        } else {
            if !deps.contains(bindings) {
                bail!(
                    "The bindings crate {} was not found in the dependencies list",
                    bindings
                );
            }

            Some(bindings.clone())
        }
    } else if deps.contains("pyo3") {
        println!("Found pyo3 bindings");
        Some("pyo3".to_string())
    } else if deps.contains("rust-cpython") {
        println!("Found rust-python bindings");
        Some("rust_cpython".to_string())
    } else {
        println!("Found cffi bindings");
        None
    };

    if let Some(bridge_crate) = bridge_crate {
        let available_versions = if !executables.is_empty() {
            PythonInterpreter::check_executables(&executables, &target)?
        } else {
            PythonInterpreter::find_all(&Target::current())?
        };

        if available_versions.is_empty() {
            bail!("Couldn't find any python interpreters. Please specify at least one with -i");
        }

        println!(
            "Found {}",
            available_versions
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>()
                .join(", ")
        );

        Ok(BridgeModel::Bindings {
            interpreter: available_versions,
            bindings_crate: bridge_crate,
        })
    } else {
        Ok(BridgeModel::Cffi)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_find_bridge() {
        let target = Target::current();
        let get_fourtytwo = cargo_metadata::metadata_deps(
            Some(&Path::new("get-fourtytwo").join("Cargo.toml")),
            true,
        ).unwrap();

        let points =
            cargo_metadata::metadata_deps(Some(&Path::new("points").join("Cargo.toml")), true)
                .unwrap();

        assert_ne!(
            find_bridge(&get_fourtytwo, &None, &[], &target).unwrap(),
            BridgeModel::Cffi
        );
        assert_ne!(
            find_bridge(&get_fourtytwo, &Some("pyo3".to_string()), &[], &target).unwrap(),
            BridgeModel::Cffi
        );

        assert_eq!(
            find_bridge(&points, &None, &[], &target).unwrap(),
            BridgeModel::Cffi
        );
        assert_eq!(
            find_bridge(&points, &Some("cffi".to_string()), &[], &target).unwrap(),
            BridgeModel::Cffi
        );

        assert!(
            find_bridge(
                &get_fourtytwo,
                &Some("rust-cpython".to_string()),
                &[],
                &target,
            ).is_err()
        );

        assert!(find_bridge(&points, &Some("rust-cpython".to_string()), &[], &target).is_err());
        assert!(find_bridge(&points, &Some("pyo3".to_string()), &[], &target).is_err());
    }
}
