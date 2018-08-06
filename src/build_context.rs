#[cfg(feature = "auditwheel")]
use auditwheel_rs;
use build_rust;
use cargo_metadata;
use cargo_toml::CargoTomlMetadata;
use cargo_toml::CargoTomlMetadataPyo3Pack;
use failure::{Error, ResultExt};
use metadata::WheelMetadata;
use std::collections::HashMap;
use std::fs::create_dir_all;
use std::fs::read_to_string;
use std::path::PathBuf;
use toml;
use wheel::build_wheel;
use CargoToml;
use Metadata21;
use PythonInterpreter;

/// Since there is no known way to list the installed python versions platform independent (or just
/// generally to list all binaries in $PATH, which could then be filtered down),
/// this is a workaround (which works until python 4 is released, which won't be too soon)
const PYTHON_INTERPRETER: &[&str] = &[
    "python2.7",
    "python3.5",
    "python3.6",
    "python3.7",
    "python3.8",
    "python3.9",
];

/// The successful return type of [build_wheels]
pub type Wheels = (Vec<(PathBuf, Option<PythonInterpreter>)>, WheelMetadata);

/// High level API for building wheels from a crate which can be also used for the CLI
#[derive(Debug, Serialize, Deserialize, StructOpt, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct BuildContext {
    #[structopt(short = "i", long = "interpreter")]
    /// The python versions to build wheels for, given as the names of the interpreters.
    /// Uses a built-in list if not explicitly set.
    pub interpreter: Vec<String>,
    /// The crate providing the python bindings
    #[structopt(short = "b", long = "bindings-crate", default_value = "pyo3")]
    pub binding_crate: String,
    #[structopt(
        short = "m",
        long = "manifest-path",
        parse(from_os_str),
        default_value = "Cargo.toml"
    )]
    /// The path to the Cargo.toml
    pub manifest_path: PathBuf,
    /// The directory to store the built wheels in. Defaults to a new "wheels" directory in the
    /// project's target directory
    #[structopt(short = "w", long = "wheel-dir", parse(from_os_str))]
    pub wheel_dir: Option<PathBuf>,
    /// Don't rebuild if a wheel with the same name is already present
    #[structopt(long = "use-cached")]
    pub use_cached: bool,
    /// Do a debug build (don't pass --release to cargo)
    #[structopt(short = "d", long = "debug")]
    pub debug: bool,
    /// Don't check for manylinux compliance
    #[structopt(long = "skip-auditwheel")]
    pub skip_auditwheel: bool,
}

impl Default for BuildContext {
    fn default() -> Self {
        BuildContext {
            interpreter: PYTHON_INTERPRETER.iter().map(ToString::to_string).collect(),
            binding_crate: "pyo3".to_string(),
            manifest_path: PathBuf::from("Cargo.toml"),
            wheel_dir: None,
            use_cached: false,
            debug: false,
            skip_auditwheel: false,
        }
    }
}

impl BuildContext {
    /// Builds wheels for a Cargo project for all given python versions. Returns the paths where
    /// the wheels are saved and the Python metadata describing the cargo project
    ///
    /// Defaults to 2.7 and 3.{5, 6, 7, 8, 9} if no python versions are given and silently
    /// ignores all non-existent python versions. Runs [auditwheel_rs()].if the auditwheel feature
    /// isn't deactivated
    pub fn build_wheels(self) -> Result<Wheels, Error> {
        let manifest_file = self.manifest_path.canonicalize().unwrap();

        if !manifest_file.is_file() {
            bail!("{} must be a path to a Cargo.toml", manifest_file.display());
        };

        let contents = read_to_string(&manifest_file).context(format!(
            "Can't read Cargo.toml at {}",
            manifest_file.display(),
        ))?;

        let cargo_toml: CargoToml =
            toml::from_str(&contents).context("Failed to parse Cargo.toml")?;
        let manifest_dir = manifest_file.parent().unwrap();
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

        // If the package name contains minuses, you must declare a module with underscores
        // as lib name
        let module_name = cargo_toml
            .lib
            .name
            .clone()
            .unwrap_or_else(|| cargo_toml.package.name.clone())
            .to_owned();

        let metadata = WheelMetadata {
            metadata21,
            scripts,
            module_name,
        };

        let available_version = if !self.interpreter.is_empty() {
            PythonInterpreter::find_all(&self.interpreter)?
        } else {
            let default_vec: Vec<_> = PYTHON_INTERPRETER.iter().map(ToString::to_string).collect();
            PythonInterpreter::find_all(&default_vec)?
        };

        if available_version.is_empty() {
            bail!("Couldn't find any python interpreters. Please specify at least one with -i");
        }

        println!(
            "Found {}",
            available_version
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        );

        let wheel_dir;
        match self.wheel_dir.clone() {
            Some(dir) => wheel_dir = dir,
            None => {
                // Failure fails here since cargo_toml does some weird stuff on their side
                let cargo_toml = cargo_metadata::metadata(Some(&manifest_file))
                    .map_err(|e| format_err!("Cargo metadata failed: {}", e))?;
                wheel_dir = PathBuf::from(cargo_toml.target_directory).join("wheels");
            }
        }

        create_dir_all(&wheel_dir)
            .context("Failed to create the target directory for the wheels")?;

        let mut wheels = Vec::new();
        for python_version in available_version {
            let wheel_path = wheel_dir.join(format!(
                "{}-{}-{}.whl",
                &metadata.metadata21.get_distribution_escaped(),
                &metadata.metadata21.version,
                python_version.get_tag()
            ));

            if self.use_cached && wheel_path.exists() {
                println!("Using cached wheel for {}", &python_version);
                wheels.push((wheel_path, Some(python_version)));
                continue;
            }

            let artifact = build_rust(
                &metadata.module_name,
                &manifest_file,
                &self,
                &python_version,
            ).context("Failed to build a native library through cargo")?;
            if !self.skip_auditwheel && python_version.target == "linux" {
                #[cfg(feature = "auditwheel")]
                auditwheel_rs(&artifact).context("Failed to ensure manylinux compliance")?;
            }
            build_wheel(&metadata, &python_version, &artifact, &wheel_path)?;

            wheels.push((wheel_path, Some(python_version)));
        }

        #[cfg(feature = "sdist")]
        {
            let sdist_path = wheel_dir.join(format!(
                "{}-{}.tar.gz",
                &metadata.metadata21.name, &metadata.metadata21.version
            ));

            println!(
                "Building the source distribution to {}",
                sdist_path.display()
            );
            ::build_source_distribution(&self, &metadata, &sdist_path)
                .context("Failed to build the source distribution")?;

            wheels.push((sdist_path, None));
        }

        Ok((wheels, metadata))
    }
}
