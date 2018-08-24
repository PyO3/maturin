#[cfg(feature = "auditwheel")]
use auditwheel_rs;
#[cfg(feature = "sdist")]
use build_source_distribution;
use cargo_metadata;
use cargo_toml::CargoTomlMetadata;
use cargo_toml::CargoTomlMetadataPyo3Pack;
use compile;
use failure::{Error, ResultExt};
use metadata::WheelMetadata;
use std::collections::HashMap;
use std::fs::create_dir_all;
use std::fs::read_to_string;
use std::path::PathBuf;
use target_info::Target;
use toml;
use wheel::build_wheel;
use CargoToml;
use Metadata21;
use PythonInterpreter;

/// The successful return type of [build_wheels]
pub type Wheels = (Vec<(PathBuf, Option<PythonInterpreter>)>, WheelMetadata);

/// High level API for building wheels from a crate which can be also used for
/// the CLI
#[derive(Debug, Serialize, Deserialize, StructOpt, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct BuildContext {
    #[structopt(short = "i", long = "interpreter")]
    /// The python versions to build wheels for, given as the names of the
    /// interpreters. Uses a built-in list if not explicitly set.
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
    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
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
    /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
    #[structopt(long = "cargo-extra-args")]
    pub cargo_extra_args: Vec<String>,
    /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
    #[structopt(long = "rustc-extra-args")]
    pub rustc_extra_args: Vec<String>,
}

impl Default for BuildContext {
    fn default() -> Self {
        BuildContext {
            interpreter: vec![],
            binding_crate: "pyo3".to_string(),
            manifest_path: PathBuf::from("Cargo.toml"),
            wheel_dir: None,
            use_cached: false,
            debug: false,
            skip_auditwheel: false,
            cargo_extra_args: Vec::new(),
            rustc_extra_args: Vec::new(),
        }
    }
}

impl BuildContext {
    /// Fills the values of [WheelMetadata]
    pub fn get_wheel_metadata(&self) -> Result<WheelMetadata, Error> {
        let manifest_file = self.manifest_path.canonicalize().unwrap();
        if !self.manifest_path.is_file() {
            bail!("{} must be a path to a Cargo.toml", manifest_file.display());
        };
        let contents = read_to_string(&manifest_file).context(format!(
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

        let metadata = WheelMetadata {
            metadata21,
            scripts,
            module_name,
        };

        Ok(metadata)
    }

    /// Builds wheels for a Cargo project for all given python versions.
    /// Returns the paths where the wheels are saved and the Python
    /// metadata describing the cargo project
    ///
    /// Defaults to 2.7 and 3.{5, 6, 7, 8, 9} if no python versions are given
    /// and silently ignores all non-existent python versions. Runs
    /// [auditwheel_rs()].if the auditwheel feature isn't deactivated
    pub fn build_wheels(&self) -> Result<Wheels, Error> {
        let metadata = self.get_wheel_metadata()?;

        let available_versions = if !self.interpreter.is_empty() {
            PythonInterpreter::check_executables(&self.interpreter)?
        } else {
            let pointer_width = match Target::pointer_width() {
                "32" => 32,
                "64" => 64,
                _ => panic!("{} is a pretty odd pointer width", Target::pointer_width()),
            };
            PythonInterpreter::find_all(&Target::os(), pointer_width)?
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

        let wheel_dir;
        match self.wheel_dir.clone() {
            Some(dir) => wheel_dir = dir,
            None => {
                // Failure fails here since cargo_toml does some weird stuff on their side
                let cargo_metadata = cargo_metadata::metadata(Some(&self.manifest_path))
                    .map_err(|e| format_err!("Cargo metadata failed: {}", e))?;
                wheel_dir = PathBuf::from(cargo_metadata.target_directory).join("wheels");
            }
        }

        create_dir_all(&wheel_dir)
            .context("Failed to create the target directory for the wheels")?;

        let mut wheels = Vec::new();
        for python_version in available_versions {
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

            let artifact = compile(
                &metadata.module_name,
                &self.manifest_path,
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
            build_source_distribution(&self, &metadata, &sdist_path)
                .context("Failed to build the source distribution")?;

            wheels.push((sdist_path, None));
        }

        Ok((wheels, metadata))
    }
}
