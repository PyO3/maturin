#[cfg(feature = "auditwheel")]
use crate::auditwheel::auditwheel_rs;
use crate::compile;
use crate::compile::warn_missing_py_init;
use crate::module_writer::WheelWriter;
use crate::module_writer::{write_bin, write_bindings_module, write_cffi_module};
use crate::Manylinux;
use crate::Metadata21;
use crate::PythonInterpreter;
use crate::Target;
use cargo_metadata::Metadata;
use failure::{bail, Context, Error, ResultExt};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// The way the rust code is used in the wheel
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeModel {
    /// A native module with c bindings, i.e. `#[no_mangle] extern "C" <some item>`
    Cffi,
    /// A rust binary to be shipped a python package
    Bin,
    /// A native module with pyo3 or rust-cpython bindings. The String is the name of the bindings
    /// providing crate, e.g. pyo3.
    Bindings(String),
}

impl BridgeModel {
    /// Returns the name of the bindings crate
    pub fn unwrap_bindings(&self) -> &str {
        match self {
            BridgeModel::Bindings(value) => &value,
            _ => panic!("Expected Bindings"),
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
    /// Pass --release to cargo
    pub release: bool,
    /// Strip the library for minimum file size
    pub strip: bool,
    /// Whether to use the the manylinux and check compliance (on), use it but don't
    /// check compliance (no-auditwheel) or use the native linux tag (off)
    pub manylinux: Manylinux,
    /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
    pub cargo_extra_args: Vec<String>,
    /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
    pub rustc_extra_args: Vec<String>,
    /// The available python interpreter
    pub interpreter: Vec<PythonInterpreter>,
    /// Cargo.toml as resolved by [cargo_metadata]
    pub cargo_metadata: Metadata,
}

impl BuildContext {
    /// Checks which kind of bindings we have (pyo3/rust-cypthon or cffi or bin) and calls the
    /// correct builder. Returns a Vec that contains location, python tag (e.g. py2.py3 or cp35)
    /// and for bindings the python interpreter they bind against.
    pub fn build_wheels(&self) -> Result<Vec<(PathBuf, String, Option<PythonInterpreter>)>, Error> {
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the wheels")?;

        let wheels = match &self.bridge {
            BridgeModel::Cffi => vec![(self.build_cffi_wheel()?, "py2.py3".to_string(), None)],
            BridgeModel::Bin => vec![(self.build_bin_wheel()?, "py2.py3".to_string(), None)],
            BridgeModel::Bindings(_) => self.build_binding_wheels()?,
        };

        Ok(wheels)
    }

    /// Builds wheels for a Cargo project for all given python versions.
    /// Return type is the same as [BuildContext::build_wheels()]
    ///
    /// Defaults to 2.7 and 3.{5, 6, 7, 8, 9} if no python versions are given
    /// and silently ignores all non-existent python versions.
    ///
    /// Runs [auditwheel_rs()] if not deactivated
    pub fn build_binding_wheels(
        &self,
    ) -> Result<Vec<(PathBuf, String, Option<PythonInterpreter>)>, Error> {
        let mut wheels = Vec::new();
        for python_version in &self.interpreter {
            let artifact = self.compile_cdylib(Some(&python_version), Some(&self.module_name))?;

            let tag = python_version.get_tag(&self.manylinux);

            let mut builder = WheelWriter::new(
                &tag,
                &self.out,
                &self.metadata21,
                &self.scripts,
                &[tag.clone()],
            )?;

            write_bindings_module(&mut builder, &self.module_name, &artifact, &python_version)?;

            let wheel_path = builder.finish()?;

            println!(
                "📦 Built wheel for {} {}.{}{} to {}",
                python_version.interpreter,
                python_version.major,
                python_version.minor,
                python_version.abiflags,
                wheel_path.display()
            );

            wheels.push((
                wheel_path,
                format!("cp{}{}", python_version.major, python_version.minor),
                Some(python_version.clone()),
            ));
        }

        Ok(wheels)
    }

    /// Runs cargo build, extracts the cdylib from the output, runs auditwheel and returns the
    /// artifact
    ///
    /// The module name is used to warn about missing a `PyInit_<module name>` function for
    /// bindings modules.
    pub fn compile_cdylib(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        module_name: Option<&str>,
    ) -> Result<PathBuf, Error> {
        let artifacts = compile(&self, python_interpreter, &self.bridge)
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

        #[cfg(feature = "auditwheel")]
        auditwheel_rs(&artifact, target, &self.manylinux)
            .context("Failed to ensure manylinux compliance")?;

        if let Some(module_name) = module_name {
            warn_missing_py_init(&artifact, module_name)
                .context("Failed to parse the native library")?;
        }

        Ok(artifact)
    }

    fn get_unversal_tags(&self) -> (String, Vec<String>) {
        let tag = format!(
            "py2.py3-none-{platform}",
            platform = self.target.get_platform_tag(&self.manylinux)
        );
        let tags = self.target.get_py2_and_py3_tags(&self.manylinux);
        (tag, tags)
    }

    /// Builds a wheel with cffi bindings
    pub fn build_cffi_wheel(&self) -> Result<PathBuf, Error> {
        let artifact = self.compile_cdylib(None, None)?;

        let (tag, tags) = self.get_unversal_tags();

        let mut builder =
            WheelWriter::new(&tag, &self.out, &self.metadata21, &self.scripts, &tags)?;

        write_cffi_module(
            &mut builder,
            self.manifest_path.parent().unwrap(),
            &self.module_name,
            &artifact,
            &self.interpreter[0].executable,
        )?;

        let wheel_path = builder.finish()?;

        println!("📦 Built wheel to {}", wheel_path.display());

        Ok(wheel_path)
    }

    /// Builds a wheel that contains a binary
    ///
    /// Runs [auditwheel_rs()] if not deactivated
    pub fn build_bin_wheel(&self) -> Result<PathBuf, Error> {
        let artifacts = compile(&self, None, &self.bridge)
            .context("Failed to build a native library through cargo")?;

        let artifact = artifacts
            .get("bin")
            .cloned()
            .ok_or_else(|| Context::new("Cargo didn't build a binary."))?;

        #[cfg(feature = "auditwheel")]
        auditwheel_rs(&artifact, &self.target, &self.manylinux)
            .context("Failed to ensure manylinux compliance")?;

        let (tag, tags) = self.get_unversal_tags();

        if !self.scripts.is_empty() {
            bail!("Defining entrypoints and working with a binary doesn't mix well");
        }

        let mut builder =
            WheelWriter::new(&tag, &self.out, &self.metadata21, &self.scripts, &tags)?;

        // I wouldn't know of any case where this would be the wrong (and neither do
        // I know a better alternative)
        let bin_name = artifact
            .file_name()
            .expect("Couldn't get the filename from the binary produced by cargo");
        write_bin(&mut builder, &artifact, &self.metadata21, bin_name)?;

        let wheel_path = builder.finish()?;

        println!("📦 Built wheel to {}", wheel_path.display());

        Ok(wheel_path)
    }
}
