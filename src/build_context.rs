#[cfg(feature = "auditwheel")]
use auditwheel_rs;
#[cfg(feature = "sdist")]
use build_source_distribution;
use compile;
use failure::{Context, Error, ResultExt};
use module_writer::WheelWriter;
use module_writer::{write_bin, write_bindings_module, write_cffi_module};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use Metadata21;
use PythonInterpreter;
use Target;

/// The way the rust code is bridged with python, i.e. either using extern c and cffi or
/// pyo3/rust-cpython
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeModel {
    /// A native module with c bindings, i.e. `#[no_mangle] extern "C" <some item>`
    Cffi,
    /// A rust binary to be shipped a python package
    Bin,
    /// A native module with pyo3 or rust-cpython bindings
    Bindings {
        interpreter: Vec<PythonInterpreter>,
        bindings_crate: String,
    },
}

impl BridgeModel {
    // TODO
    pub fn to_option(&self) -> Option<String> {
        match self {
            BridgeModel::Cffi | BridgeModel::Bin => None,
            BridgeModel::Bindings { bindings_crate, .. } => Some(bindings_crate.clone()),
        }
    }
}

/// Contains all the metadata required to build the crate
pub struct BuildContext {
    /// The platform, i.e. os and pointer width
    pub target: Target,
    /// Whether to use cffi or pyo3/rust-cpython
    pub bridge: BridgeModel,
    /// Python Package Metadata 2.1
    pub metadata21: Metadata21,
    /// The `[console_scripts]` for the entry_points.txt
    pub scripts: HashMap<String, String>,
    /// The name of the module can be distinct from the package name, mostly
    /// because package names normally contain minuses while module names
    /// have underscores. The package name is part of metadata21
    pub module_name: String,
    /// The path to the Cargo.toml. Required for the cargo invocations
    pub manifest_path: PathBuf,
    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    pub out: PathBuf,
    /// Do a debug build (don't pass --release to cargo)
    pub debug: bool,
    /// Don't check for manylinux compliance
    pub skip_auditwheel: bool,
    /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
    pub cargo_extra_args: Vec<String>,
    /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
    pub rustc_extra_args: Vec<String>,
}

impl BuildContext {
    /// Checks which kind of bindings we have (pyo3/rust-cypthon vs cffi) and calls the correct
    /// builder
    pub fn build_wheels(&self) -> Result<Vec<(PathBuf, String, Option<PythonInterpreter>)>, Error> {
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the wheels")?;

        let wheels = match &self.bridge {
            BridgeModel::Cffi => vec![(self.build_cffi_wheel()?, "py2.py3".to_string() , None)],
            BridgeModel::Bin => vec![(self.build_bin_wheel()?, "py2.py3".to_string(), None)],
            BridgeModel::Bindings { interpreter, .. } => self.build_binding_wheels(&interpreter)?,
        };

        Ok(wheels)
    }

    /// Builds wheels for a Cargo project for all given python versions.
    /// Returns the paths where the wheels are saved and the Python
    /// metadata describing the cargo project
    ///
    /// Defaults to 2.7 and 3.{5, 6, 7, 8, 9} if no python versions are given
    /// and silently ignores all non-existent python versions. Runs
    /// [auditwheel_rs()] if the auditwheel feature isn't deactivated
    pub fn build_binding_wheels(
        &self,
        interpreter: &[PythonInterpreter],
    ) -> Result<Vec<(PathBuf, String, Option<PythonInterpreter>)>, Error> {
        let mut wheels = Vec::new();
        for python_version in interpreter {
            let artifact = self.compile_cdylib(Some(&python_version))?;

            let tag = python_version.get_tag();

            let mut builder =
                WheelWriter::new(&tag, &self.out, &self.metadata21, &self.scripts, &[&tag])?;

            write_bindings_module(&mut builder, &self.module_name, &artifact, &python_version)?;

            let wheel_path = builder.finish()?;

            println!(
                "Built wheel for python {}.{}{} to {}",
                python_version.major,
                python_version.minor,
                python_version.abiflags,
                wheel_path.display()
            );

            wheels.push((wheel_path, format!("cp{}{}", python_version.major, python_version.minor), Some(python_version.clone())));
        }

        #[cfg(feature = "sdist")]
        {
            let sdist_path = wheel_dir.join(format!(
                "{}-{}.tar.gz",
                &self.metadata21.get_distribution_encoded(),
                &self.metadata21.get_version_encoded()
            ));

            println!(
                "Building the source distribution to {}",
                sdist_path.display()
            );
            build_source_distribution(&self, &self.metadata21, &self.scripts, &sdist_path)
                .context("Failed to build the source distribution")?;

            wheels.push((sdist_path, None));
        }

        Ok(wheels)
    }

    /// Runs cargo build, extracts the cdylib from the output, runs auditwheel and returns the
    /// artifact
    pub fn compile_cdylib(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
    ) -> Result<PathBuf, Error> {
        let artifacts = compile(&self, python_interpreter, self.bridge.to_option())
            .context("Failed to build a native library through cargo")?;

        let artifact = artifacts.get("cdylib").cloned().ok_or_else(|| {
            Context::new(
                "Cargo didn't build a cdylib. Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?",
            )
        })?;

        let target = python_interpreter
            .map(|x| &x.target)
            .unwrap_or(&self.target);

        if !self.skip_auditwheel && target.is_linux() {
            #[cfg(feature = "auditwheel")]
            auditwheel_rs(&artifact).context("Failed to ensure manylinux compliance")?;
        }

        Ok(artifact)
    }

    fn get_unversal_tags(&self) -> (String, &[&'static str]) {
        let tag = format!(
            "py2.py3-none-{platform}",
            platform = self.target.get_platform_tag()
        );
        let tags = self.target.get_cffi_tags();
        (tag, tags)
    }

    /// Builds a wheel with cffi bindings
    pub fn build_cffi_wheel(&self) -> Result<PathBuf, Error> {
        let artifact = self.compile_cdylib(None)?;

        let (tag, tags) = self.get_unversal_tags();

        let mut builder = WheelWriter::new(&tag, &self.out, &self.metadata21, &self.scripts, tags)?;

        write_cffi_module(&mut builder, &self.module_name, &artifact, &self.target)?;

        let wheel_path = builder.finish()?;

        println!("Built wheel to {}", wheel_path.display());

        Ok(wheel_path)
    }

    /// Builds a wheel that contains a rust binary and an entrypoint for that binary
    pub fn build_bin_wheel(&self) -> Result<PathBuf, Error> {
        let artifacts = compile(&self, None, self.bridge.to_option())
            .context("Failed to build a native library through cargo")?;

        let artifact = artifacts
            .get("bin")
            .cloned()
            .ok_or_else(|| Context::new("Cargo didn't build a binary."))?;

        if !self.skip_auditwheel && self.target.is_linux() {
            #[cfg(feature = "auditwheel")]
            auditwheel_rs(&artifact).context("Failed to ensure manylinux compliance")?;
        }

        let (tag, tags) = self.get_unversal_tags();

        if !self.scripts.is_empty() {
            bail!("Defining entrypoints and working with a binary doesn't mix well");
        }

        let mut builder = WheelWriter::new(&tag, &self.out, &self.metadata21, &self.scripts, tags)?;

        // I wouldn't know of any case where this would be the wrong (and neither do
        // I know a better alternative)
        let bin_name = artifact
            .file_name()
            .expect("Couldn't get the filename from the binary produced by cargo");
        write_bin(&mut builder, &artifact, &self.metadata21, bin_name)?;

        let wheel_path = builder.finish()?;

        println!("Built wheel to {}", wheel_path.display());

        Ok(wheel_path)
    }
}
