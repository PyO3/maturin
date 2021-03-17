use crate::auditwheel::auditwheel_rs;
use crate::auditwheel::policy::Policy;
use crate::compile;
use crate::compile::warn_missing_py_init;
use crate::module_writer::write_python_part;
use crate::module_writer::WheelWriter;
use crate::module_writer::{write_bin, write_bindings_module, write_cffi_module};
use crate::source_distribution::{get_pyproject_toml, source_distribution};
use crate::Manylinux;
use crate::Metadata21;
use crate::PythonInterpreter;
use crate::Target;
use anyhow::{anyhow, bail, Context, Result};
use cargo_metadata::Metadata;
use fs_err as fs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// `Bindings`, but specifically for pyo3 with feature flags that allow building a single wheel
    /// for all cpython versions (pypy still needs multiple versions).
    /// The numbers are the minimum major and minor version
    BindingsAbi3(u8, u8),
}

impl BridgeModel {
    /// Returns the name of the bindings crate
    pub fn unwrap_bindings(&self) -> &str {
        match self {
            BridgeModel::Bindings(value) => &value,
            _ => panic!("Expected Bindings"),
        }
    }

    /// Test whether this is using a specific bindings crate
    pub fn is_bindings(&self, name: &str) -> bool {
        match self {
            BridgeModel::Bindings(value) => value == name,
            _ => false,
        }
    }
}

/// Whether this project is pure rust or rust mixed with python
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectLayout {
    /// A rust crate compiled into a shared library with only some glue python for cffi
    PureRust,
    /// A python package that is extended by a native rust module.
    ///
    /// Contains the canonicialized (i.e. absolute) path to the python part of the project
    Mixed(PathBuf),
}

impl ProjectLayout {
    /// Checks whether a python module exists besides Cargo.toml with the right name
    pub fn determine(project_root: impl AsRef<Path>, module_name: &str) -> Result<ProjectLayout> {
        let python_package_dir = project_root.as_ref().join(module_name);
        if python_package_dir.is_dir() {
            if !python_package_dir.join("__init__.py").is_file()
                && !python_package_dir.join("__init__.pyi").is_file()
            {
                bail!("Found a directory with the module name ({}) next to Cargo.toml, which indicates a mixed python/rust project, but the directory didn't contain an __init__.{{py,pyi}} file.", module_name)
            }

            println!("🍹 Building a mixed python/rust project");

            Ok(ProjectLayout::Mixed(python_package_dir))
        } else {
            Ok(ProjectLayout::PureRust)
        }
    }
}

/// Contains all the metadata required to build the crate
pub struct BuildContext {
    /// The platform, i.e. os and pointer width
    pub target: Target,
    /// Whether to use cffi or pyo3/rust-cpython
    pub bridge: BridgeModel,
    /// Whether this project is pure rust or rust mixed with python
    pub project_layout: ProjectLayout,
    /// Python Package Metadata 2.1
    pub metadata21: Metadata21,
    /// The `[console_scripts]` for the entry_points.txt
    pub scripts: HashMap<String, String>,
    /// The name of the crate
    pub crate_name: String,
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
    /// Whether to skip checking the linked libraries for manylinux compliance
    pub skip_auditwheel: bool,
    /// Whether to use the the manylinux or use the native linux tag (off)
    pub manylinux: Manylinux,
    /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
    pub cargo_extra_args: Vec<String>,
    /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
    pub rustc_extra_args: Vec<String>,
    /// The available python interpreter
    pub interpreter: Vec<PythonInterpreter>,
    /// Cargo.toml as resolved by [cargo_metadata]
    pub cargo_metadata: Metadata,
    /// Whether to use universal2 or use the native macOS tag (off)
    pub universal2: bool,
}

type BuiltWheelMetadata = (PathBuf, String);

impl BuildContext {
    /// Checks which kind of bindings we have (pyo3/rust-cypthon or cffi or bin) and calls the
    /// correct builder. Returns a Vec that contains location, python tag (e.g. py3 or cp35)
    /// and for bindings the python interpreter they bind against.
    pub fn build_wheels(&self) -> Result<Vec<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the wheels")?;

        let wheels = match &self.bridge {
            BridgeModel::Cffi => self.build_cffi_wheel()?,
            BridgeModel::Bin => self.build_bin_wheel()?,
            BridgeModel::Bindings(_) => self.build_binding_wheels()?,
            BridgeModel::BindingsAbi3(major, minor) => {
                self.build_binding_wheel_abi3(*major, *minor)?
            }
        };

        Ok(wheels)
    }

    /// Builds a source distribution and returns the same metadata as [BuildContext::build_wheels]
    pub fn build_source_distribution(&self) -> Result<Option<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the source distribution")?;

        match get_pyproject_toml(self.manifest_path.parent().unwrap()) {
            Ok(pyproject) => {
                let sdist_path = source_distribution(
                    &self.out,
                    &self.metadata21,
                    &self.manifest_path,
                    &self.cargo_metadata,
                    pyproject.sdist_include(),
                )
                .context("Failed to build source distribution")?;
                Ok(Some((sdist_path, "source".to_string())))
            }
            Err(_) => Ok(None),
        }
    }

    fn auditwheel(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        artifact: &Path,
        manylinux: &Manylinux,
    ) -> Result<Option<Policy>> {
        if !self.skip_auditwheel {
            let target = python_interpreter
                .map(|x| &x.target)
                .unwrap_or(&self.target);

            let policy = auditwheel_rs(&artifact, target, manylinux)
                .context(format!("Failed to ensure {} compliance", manylinux))?;
            Ok(policy)
        } else {
            Ok(None)
        }
    }

    fn write_binding_wheel_abi3(
        &self,
        artifact: &Path,
        manylinux: &Manylinux,
        major: u8,
        min_minor: u8,
    ) -> Result<(PathBuf, String)> {
        let platform = self.target.get_platform_tag(&manylinux, self.universal2);
        let tag = format!("cp{}{}-abi3-{}", major, min_minor, platform);

        let mut writer = WheelWriter::new(
            &tag,
            &self.out,
            &self.metadata21,
            &self.scripts,
            &[tag.clone()],
        )?;

        write_bindings_module(
            &mut writer,
            &self.project_layout,
            &self.module_name,
            artifact,
            None,
            &self.target,
            false,
        )
        .context("Failed to add the files to the wheel")?;

        let wheel_path = writer.finish()?;
        Ok((wheel_path, format!("cp{}{}", major, min_minor)))
    }

    /// For abi3 we only need to build a single wheel and we don't even need a python interpreter
    /// for it
    pub fn build_binding_wheel_abi3(
        &self,
        major: u8,
        min_minor: u8,
    ) -> Result<Vec<(PathBuf, String)>> {
        let mut wheels = Vec::new();
        // On windows, we have picked an interpreter to set the location of python.lib,
        // otherwise it's none
        let python_interpreter = self.interpreter.get(0);
        let artifact = self.compile_cdylib(python_interpreter, Some(&self.module_name))?;
        let policy = self.auditwheel(python_interpreter, &artifact, &self.manylinux)?;
        let (wheel_path, tag) =
            self.write_binding_wheel_abi3(&artifact, &self.manylinux, major, min_minor)?;

        println!(
            "📦 Built wheel for abi3 Python ≥ {}.{} to {}",
            major,
            min_minor,
            wheel_path.display()
        );
        wheels.push((wheel_path, tag));

        if let Some(policy) = policy {
            if policy > Policy::from_name(&self.manylinux.to_string()).unwrap() {
                let manylinux = policy.manylinux_tag();
                if self
                    .auditwheel(python_interpreter, &artifact, &manylinux)
                    .is_ok()
                {
                    println!(
                        "📦 Wheel is eligible for a higher priority tag. You requested {} but I have found this wheel is eligible for {}",
                        &self.manylinux,
                        manylinux,
                    );
                    let (wheel_path, tag) =
                        self.write_binding_wheel_abi3(&artifact, &manylinux, major, min_minor)?;
                    println!("📦 Fixed-up wheel written to {}", wheel_path.display());
                    wheels.push((wheel_path, tag));
                }
            }
        }

        Ok(wheels)
    }

    fn write_binding_wheel(
        &self,
        python_interpreter: &PythonInterpreter,
        artifact: &Path,
        manylinux: &Manylinux,
    ) -> Result<(PathBuf, String)> {
        let tag = python_interpreter.get_tag(manylinux, self.universal2);

        let mut writer = WheelWriter::new(
            &tag,
            &self.out,
            &self.metadata21,
            &self.scripts,
            &[tag.clone()],
        )?;

        write_bindings_module(
            &mut writer,
            &self.project_layout,
            &self.module_name,
            &artifact,
            Some(&python_interpreter),
            &self.target,
            false,
        )
        .context("Failed to add the files to the wheel")?;

        let wheel_path = writer.finish()?;
        Ok((
            wheel_path,
            format!("cp{}{}", python_interpreter.major, python_interpreter.minor),
        ))
    }

    /// Builds wheels for a Cargo project for all given python versions.
    /// Return type is the same as [BuildContext::build_wheels()]
    ///
    /// Defaults to 3.{5, 6, 7, 8, 9} if no python versions are given
    /// and silently ignores all non-existent python versions.
    ///
    /// Runs [auditwheel_rs()] if not deactivated
    pub fn build_binding_wheels(&self) -> Result<Vec<(PathBuf, String)>> {
        let mut wheels = Vec::new();
        for python_interpreter in &self.interpreter {
            let artifact =
                self.compile_cdylib(Some(&python_interpreter), Some(&self.module_name))?;
            let policy = self.auditwheel(Some(&python_interpreter), &artifact, &self.manylinux)?;
            let (wheel_path, tag) =
                self.write_binding_wheel(python_interpreter, &artifact, &self.manylinux)?;
            println!(
                "📦 Built wheel for {} {}.{}{} to {}",
                python_interpreter.interpreter_kind,
                python_interpreter.major,
                python_interpreter.minor,
                python_interpreter.abiflags,
                wheel_path.display()
            );

            wheels.push((wheel_path, tag));

            if let Some(policy) = policy {
                if policy > Policy::from_name(&self.manylinux.to_string()).unwrap() {
                    let manylinux = policy.manylinux_tag();
                    if self
                        .auditwheel(Some(&python_interpreter), &artifact, &manylinux)
                        .is_ok()
                    {
                        println!(
                            "📦 Wheel is eligible for a higher priority tag. You requested {} but I have found this wheel is eligible for {}",
                            &self.manylinux,
                            manylinux,
                        );
                        let (wheel_path, tag) =
                            self.write_binding_wheel(python_interpreter, &artifact, &manylinux)?;
                        println!("📦 Fixed-up wheel written to {}", wheel_path.display());
                        wheels.push((wheel_path, tag));
                    }
                }
            }
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
    ) -> Result<PathBuf> {
        let artifacts = compile(&self, python_interpreter, &self.bridge)
            .context("Failed to build a native library through cargo")?;

        let artifact = artifacts.get("cdylib").cloned().ok_or_else(|| {
            anyhow!(
                "Cargo didn't build a cdylib. Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?",
            )
        })?;

        if let Some(module_name) = module_name {
            warn_missing_py_init(&artifact, module_name)
                .context("Failed to parse the native library")?;
        }

        Ok(artifact)
    }

    fn write_cffi_wheel(
        &self,
        artifact: &Path,
        manylinux: &Manylinux,
    ) -> Result<(PathBuf, String)> {
        let (tag, tags) = self.target.get_universal_tags(manylinux, self.universal2);

        let mut builder =
            WheelWriter::new(&tag, &self.out, &self.metadata21, &self.scripts, &tags)?;

        write_cffi_module(
            &mut builder,
            &self.project_layout,
            self.manifest_path.parent().unwrap(),
            &self.module_name,
            &artifact,
            &self.interpreter[0].executable,
            false,
        )?;

        let wheel_path = builder.finish()?;
        Ok((wheel_path, "py3".to_string()))
    }

    /// Builds a wheel with cffi bindings
    pub fn build_cffi_wheel(&self) -> Result<Vec<(PathBuf, String)>> {
        let mut wheels = Vec::new();
        let artifact = self.compile_cdylib(None, None)?;
        let (wheel_path, tag) = self.write_cffi_wheel(&artifact, &self.manylinux)?;
        let policy = self.auditwheel(None, &artifact, &self.manylinux)?;

        println!("📦 Built wheel to {}", wheel_path.display());
        wheels.push((wheel_path, tag));

        if let Some(policy) = policy {
            if policy > Policy::from_name(&self.manylinux.to_string()).unwrap() {
                let manylinux = policy.manylinux_tag();
                if self.auditwheel(None, &artifact, &manylinux).is_ok() {
                    println!(
                        "📦 Wheel is eligible for a higher priority tag. You requested {} but I have found this wheel is eligible for {}",
                        &self.manylinux,
                        manylinux,
                    );
                    let (wheel_path, tag) = self.write_cffi_wheel(&artifact, &manylinux)?;
                    println!("📦 Fixed-up wheel written to {}", wheel_path.display());
                    wheels.push((wheel_path, tag));
                }
            }
        }

        Ok(wheels)
    }

    fn write_bin_wheel(&self, artifact: &Path, manylinux: &Manylinux) -> Result<(PathBuf, String)> {
        let (tag, tags) = self.target.get_universal_tags(manylinux, self.universal2);

        if !self.scripts.is_empty() {
            bail!("Defining entrypoints and working with a binary doesn't mix well");
        }

        let mut builder =
            WheelWriter::new(&tag, &self.out, &self.metadata21, &self.scripts, &tags)?;

        match self.project_layout {
            ProjectLayout::Mixed(ref python_module) => {
                write_python_part(&mut builder, python_module, &self.module_name)
                    .context("Failed to add the python module to the package")?;
            }
            ProjectLayout::PureRust => {}
        }

        // I wouldn't know of any case where this would be the wrong (and neither do
        // I know a better alternative)
        let bin_name = artifact
            .file_name()
            .expect("Couldn't get the filename from the binary produced by cargo");
        write_bin(&mut builder, &artifact, &self.metadata21, bin_name)?;

        let wheel_path = builder.finish()?;
        Ok((wheel_path, "py3".to_string()))
    }

    /// Builds a wheel that contains a binary
    ///
    /// Runs [auditwheel_rs()] if not deactivated
    pub fn build_bin_wheel(&self) -> Result<Vec<(PathBuf, String)>> {
        let mut wheels = Vec::new();
        let artifacts = compile(&self, None, &self.bridge)
            .context("Failed to build a native library through cargo")?;

        let artifact = artifacts
            .get("bin")
            .cloned()
            .ok_or_else(|| anyhow!("Cargo didn't build a binary"))?;

        let policy = self.auditwheel(None, &artifact, &self.manylinux)?;

        let (wheel_path, tag) = self.write_bin_wheel(&artifact, &self.manylinux)?;
        println!("📦 Built wheel to {}", wheel_path.display());
        wheels.push((wheel_path, tag));

        if let Some(policy) = policy {
            if policy > Policy::from_name(&self.manylinux.to_string()).unwrap() {
                let manylinux = policy.manylinux_tag();
                if self.auditwheel(None, &artifact, &manylinux).is_ok() {
                    println!(
                        "📦 Wheel is eligible for a higher priority tag. You requested {} but I have found this wheel is eligible for {}",
                        &self.manylinux,
                        manylinux,
                    );
                    let (wheel_path, tag) = self.write_bin_wheel(&artifact, &manylinux)?;
                    println!("📦 Fixed-up wheel written to {}", wheel_path.display());
                    wheels.push((wheel_path, tag));
                }
            }
        }

        Ok(wheels)
    }
}
