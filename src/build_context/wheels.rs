use crate::binding_generator::{
    BinBindingGenerator, BindingGenerator, CffiBindingGenerator, Pyo3BindingGenerator,
    UniFfiBindingGenerator, generate_binding,
};
use crate::compile::warn_missing_py_init;
use crate::module_writer::{WheelWriter, add_data};
use crate::sbom::{SbomData, write_sboms};
use crate::util::zip_mtime;
use crate::{
    BridgeModel, BuildArtifact, PythonInterpreter, VirtualWriter, compile, pyproject_toml::Format,
};
use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::CrateType;
use itertools::Itertools;
use lddtree::Library;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use super::{BuildContext, BuiltWheelMetadata};
use crate::auditwheel::PlatformTag;

impl BuildContext {
    /// Split interpreters into abi3-capable and non-abi3 groups, build the
    /// appropriate wheel type for each group, and return all built wheels.
    ///
    /// When `min_version` is `Some((major, minor))` (i.e. `Abi3Version::Version`),
    /// interpreters below that version are excluded from the abi3 group.
    /// When `min_version` is `None` (i.e. `Abi3Version::CurrentPython`),
    /// all `has_stable_api()` interpreters are in the abi3 group and the
    /// baseline version is taken from the first one.
    pub(super) fn build_abi3_wheels(
        &self,
        min_version: Option<(u8, u8)>,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let abi3_interps: Vec<_> = self
            .python
            .interpreter
            .iter()
            .filter(|interp| {
                interp.has_stable_api()
                    && min_version.is_none_or(|(major, minor)| {
                        (interp.major as u8, interp.minor as u8) >= (major, minor)
                    })
            })
            .cloned()
            .collect();
        let non_abi3_interps: Vec<_> = self
            .python
            .interpreter
            .iter()
            .filter(|interp| !interp.has_stable_api())
            .cloned()
            .collect();

        let mut built_wheels = Vec::new();
        if let Some(first) = abi3_interps.first() {
            let (major, minor) = min_version.unwrap_or((first.major as u8, first.minor as u8));
            built_wheels.extend(self.build_pyo3_wheel_abi3(
                &abi3_interps,
                major,
                minor,
                sbom_data,
            )?);
        }
        if !non_abi3_interps.is_empty() {
            let interp_names: HashSet<_> = non_abi3_interps
                .iter()
                .map(|interp| interp.to_string())
                .collect();
            eprintln!(
                "⚠️ Warning: {} does not yet support abi3 so the build artifacts will be version-specific.",
                interp_names.iter().join(", ")
            );
            built_wheels.extend(self.build_pyo3_wheels(&non_abi3_interps, sbom_data)?);
        }
        Ok(built_wheels)
    }

    /// Unified wheel-building pipeline for non-bin binding types.
    ///
    /// This method handles the common 8-step pattern shared by all extension
    /// module wheels (PyO3, PyO3 abi3, CFFI, UniFfi):
    ///   1. Create WheelWriter with compression options
    ///   2. Create VirtualWriter with excludes
    ///   3. Add external shared libraries (auditwheel repair)
    ///   4. Run the binding generator (install extension + python files)
    ///   5. Write .pth file for editable installs
    ///   6. Add data directory
    ///   7. Write SBOM files
    ///   8. Finish the wheel
    ///
    /// The `make_generator` closure receives the writer's temp directory
    /// (needed by some generators for intermediate files) and returns the
    /// binding generator to use.
    #[allow(clippy::too_many_arguments, clippy::needless_lifetimes)]
    fn write_wheel<'a, F>(
        &'a self,
        tag: &str,
        artifacts: &[&BuildArtifact],
        ext_libs: &[Vec<Library>],
        make_generator: F,
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<PathBuf>
    where
        F: FnOnce(Rc<tempfile::TempDir>) -> Result<Box<dyn BindingGenerator + 'a>>,
    {
        let file_options = self
            .artifact
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let writer = WheelWriter::new(
            tag,
            &self.artifact.out,
            &self.project.metadata24,
            file_options,
        )?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);
        self.add_external_libs(&mut writer, artifacts, ext_libs)?;

        let temp_dir = writer.temp_dir()?;
        let mut generator = make_generator(temp_dir)?;
        generate_binding(&mut writer, generator.as_mut(), self, artifacts, out_dirs)
            .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &self.project.metadata24,
            self.project.project_layout.data.as_deref(),
        )?;

        write_sboms(
            &self.project,
            &self.artifact,
            sbom_data.as_ref(),
            &mut writer,
            &self.project.metadata24.get_dist_info_dir(),
        )?;

        let tags = [tag.to_string()];
        let wheel_path = writer.finish(
            &self.project.metadata24,
            &self.project.project_layout.project_root,
            &tags,
        )?;
        Ok(wheel_path)
    }

    /// For abi3 we only need to build a single wheel and we don't even need a python interpreter
    /// for it
    pub fn build_pyo3_wheel_abi3(
        &self,
        interpreters: &[PythonInterpreter],
        major: u8,
        min_minor: u8,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        // On windows, we have picked an interpreter to set the location of python.lib,
        // otherwise it's none
        let python_interpreter = interpreters.first();
        let (artifact, out_dirs) = self.compile_cdylib(
            python_interpreter,
            Some(&self.project.project_layout.extension_name),
        )?;
        let (policy, external_libs) =
            self.auditwheel(&artifact, &self.python.platform_tag, python_interpreter)?;
        let platform_tags = self.resolve_platform_tags(&policy);

        let platform = self.project.get_platform_tag(&platform_tags)?;
        let tag = format!("cp{major}{min_minor}-abi3-{platform}");

        let wheel_path = self.write_wheel(
            &tag,
            &[&artifact],
            &[external_libs],
            |temp_dir| {
                Ok(Box::new(
                    Pyo3BindingGenerator::new(true, python_interpreter, temp_dir)
                        .context("Failed to initialize PyO3 binding generator")?,
                ))
            },
            sbom_data,
            &out_dirs,
        )?;

        eprintln!(
            "📦 Built wheel for abi3 Python ≥ {}.{} to {}",
            major,
            min_minor,
            wheel_path.display()
        );
        wheels.push((wheel_path, format!("cp{major}{min_minor}")));

        Ok(wheels)
    }

    fn write_pyo3_wheel(
        &self,
        python_interpreter: &PythonInterpreter,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<PathBuf> {
        let tag = python_interpreter.get_tag(&self.project, platform_tags)?;

        self.write_wheel(
            &tag,
            &[&artifact],
            &[ext_libs],
            |temp_dir| {
                Ok(Box::new(
                    Pyo3BindingGenerator::new(false, Some(python_interpreter), temp_dir)
                        .context("Failed to initialize PyO3 binding generator")?,
                ))
            },
            sbom_data,
            out_dirs,
        )
    }

    /// Compile, audit, and write a single PyO3 wheel for one interpreter.
    ///
    /// This is the core pipeline shared by [`build_pyo3_wheels`] and the
    /// per-interpreter PGO path in [`build_wheels_pgo_per_interpreter`].
    pub(super) fn build_single_pyo3_wheel(
        &self,
        python_interpreter: &PythonInterpreter,
        sbom_data: &Option<SbomData>,
    ) -> Result<BuiltWheelMetadata> {
        let (artifact, out_dirs) = self.compile_cdylib(
            Some(python_interpreter),
            Some(&self.project.project_layout.extension_name),
        )?;
        let (policy, external_libs) = self.auditwheel(
            &artifact,
            &self.python.platform_tag,
            Some(python_interpreter),
        )?;
        let platform_tags = self.resolve_platform_tags(&policy);
        let wheel_path = self.write_pyo3_wheel(
            python_interpreter,
            artifact,
            &platform_tags,
            external_libs,
            sbom_data,
            &out_dirs,
        )?;
        let tag = format!("cp{}{}", python_interpreter.major, python_interpreter.minor);
        Ok((wheel_path, tag))
    }

    /// Builds wheels for a pyo3 extension for all given python versions.
    /// Return type is the same as [BuildContext::build_wheels()]
    ///
    /// Defaults to 3.{5, 6, 7, 8, 9} if no python versions are given
    /// and silently ignores all non-existent python versions.
    ///
    /// Runs [auditwheel_rs()] if not deactivated
    pub fn build_pyo3_wheels(
        &self,
        interpreters: &[PythonInterpreter],
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        for python_interpreter in interpreters {
            let (wheel_path, tag) = self.build_single_pyo3_wheel(python_interpreter, sbom_data)?;
            eprintln!(
                "📦 Built wheel for {} {}.{}{} to {}",
                python_interpreter.interpreter_kind,
                python_interpreter.major,
                python_interpreter.minor,
                python_interpreter.abiflags,
                wheel_path.display()
            );
            wheels.push((wheel_path, tag));
        }

        Ok(wheels)
    }

    /// Runs cargo build, extracts the cdylib from the output and returns the path to it
    ///
    /// The module name is used to warn about missing a `PyInit_<module name>` function for
    /// bindings modules.
    pub fn compile_cdylib(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        extension_name: Option<&str>,
    ) -> Result<(BuildArtifact, HashMap<String, PathBuf>)> {
        let result = compile(self, python_interpreter, &self.project.compile_targets)?;
        let error_msg = "Cargo didn't build a cdylib. Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?";
        let artifacts = result.artifacts.first().context(error_msg)?;

        let mut artifact = artifacts
            .get(&CrateType::CDyLib)
            .cloned()
            .ok_or_else(|| anyhow!(error_msg,))?;

        self.stage_artifact(&mut artifact)?;

        if let Some(extension_name) = extension_name {
            // goblin has an issue parsing MIPS64 ELF, see https://github.com/m4b/goblin/issues/274
            // But don't fail the build just because we can't emit a warning
            let _ = warn_missing_py_init(&artifact.path, extension_name);
        }
        Ok((artifact, result.out_dirs))
    }

    /// Shared build pipeline for cdylib-based bindings (CFFI, UniFfi).
    ///
    /// Compiles the cdylib, runs auditwheel, resolves platform tags, writes
    /// the wheel via `write_wheel`, and returns the built wheel metadata.
    #[allow(clippy::needless_lifetimes)] // false positive
    fn build_cdylib_wheel<'a, F>(
        &'a self,
        make_generator: F,
        sbom_data: &Option<SbomData>,
    ) -> Result<(PathBuf, HashMap<String, PathBuf>)>
    where
        F: FnOnce(Rc<tempfile::TempDir>) -> Result<Box<dyn BindingGenerator + 'a>>,
    {
        let (artifact, out_dirs) = self.compile_cdylib(None, None)?;
        let (policy, external_libs) =
            self.auditwheel(&artifact, &self.python.platform_tag, None)?;
        let platform_tags = self.resolve_platform_tags(&policy);
        let tag = self.get_universal_tag(&platform_tags)?;
        let wheel_path = self.write_wheel(
            &tag,
            &[&artifact],
            &[external_libs],
            make_generator,
            sbom_data,
            &out_dirs,
        )?;
        Ok((wheel_path, out_dirs))
    }

    /// Builds a wheel with cffi bindings
    pub fn build_cffi_wheel(
        &self,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let interpreter = self.python.interpreter.first().ok_or_else(|| {
            anyhow!("A python interpreter is required for cffi builds but one was not provided")
        })?;
        let (wheel_path, _) = self.build_cdylib_wheel(
            |temp_dir| {
                Ok(Box::new(
                    CffiBindingGenerator::new(interpreter, temp_dir)
                        .context("Failed to initialize Cffi binding generator")?,
                ))
            },
            sbom_data,
        )?;

        // Warn if cffi isn't specified in the requirements
        if !self
            .project
            .metadata24
            .requires_dist
            .iter()
            .any(|requirement| requirement.name.as_ref() == "cffi")
        {
            eprintln!(
                "⚠️  Warning: missing cffi package dependency, please add it to pyproject.toml. \
                e.g: `dependencies = [\"cffi\"]`. This will become an error."
            );
        }

        eprintln!("📦 Built wheel to {}", wheel_path.display());
        Ok(vec![(wheel_path, "py3".to_string())])
    }

    /// Builds a wheel with uniffi bindings
    pub fn build_uniffi_wheel(
        &self,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let (wheel_path, _) = self.build_cdylib_wheel(
            |_temp_dir| Ok(Box::new(UniFfiBindingGenerator::default())),
            sbom_data,
        )?;

        eprintln!("📦 Built wheel to {}", wheel_path.display());
        Ok(vec![(wheel_path, "py3".to_string())])
    }

    fn write_bin_wheel(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        artifacts: &[BuildArtifact],
        platform_tags: &[PlatformTag],
        ext_libs: &[Vec<Library>],
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<PathBuf> {
        if !self.project.metadata24.scripts.is_empty() {
            bail!("Defining scripts and working with a binary doesn't mix well");
        }

        if self.project.target.is_wasi() {
            eprintln!("⚠️  Warning: wasi support is experimental");
            if !self.project.metadata24.entry_points.is_empty() {
                bail!("You can't define entrypoints yourself for a binary project");
            }

            if self.project.project_layout.python_module.is_some() {
                // TODO: Can we have python code and the wasm launchers coexisting
                // without clashes?
                bail!("Sorry, adding python code to a wasm binary is currently not supported")
            }
        }

        let tag = match (self.project.bridge(), python_interpreter) {
            (BridgeModel::Bin(None), _) => self.get_universal_tag(platform_tags)?,
            (BridgeModel::Bin(Some(..)), Some(python_interpreter)) => {
                python_interpreter.get_tag(&self.project, platform_tags)?
            }
            _ => unreachable!(),
        };

        let mut metadata24 = self.project.metadata24.clone();
        let file_options = self
            .artifact
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let writer = WheelWriter::new(&tag, &self.artifact.out, &metadata24, file_options)?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);

        self.add_external_libs(&mut writer, artifacts, ext_libs)?;

        let mut generator = BinBindingGenerator::new(&mut metadata24);
        generate_binding(&mut writer, &mut generator, self, artifacts, out_dirs)
            .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &metadata24,
            self.project.project_layout.data.as_deref(),
        )?;
        write_sboms(
            &self.project,
            &self.artifact,
            sbom_data.as_ref(),
            &mut writer,
            &metadata24.get_dist_info_dir(),
        )?;
        let tags = [tag];
        let wheel_path = writer.finish(
            &metadata24,
            &self.project.project_layout.project_root,
            &tags,
        )?;
        Ok(wheel_path)
    }

    /// Builds a wheel that contains a binary
    ///
    /// Runs [auditwheel_rs()] if not deactivated
    pub fn build_bin_wheel(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        let result = compile(self, python_interpreter, &self.project.compile_targets)?;
        if result.artifacts.is_empty() {
            bail!("Cargo didn't build a binary")
        }

        let mut policies = Vec::with_capacity(result.artifacts.len());
        let mut ext_libs = Vec::new();
        let mut artifact_paths = Vec::with_capacity(result.artifacts.len());
        for artifact in result.artifacts {
            let mut artifact = artifact
                .get(&CrateType::Bin)
                .cloned()
                .ok_or_else(|| anyhow!("Cargo didn't build a binary"))?;

            let (policy, external_libs) =
                self.auditwheel(&artifact, &self.python.platform_tag, None)?;
            policies.push(policy);
            ext_libs.push(external_libs);

            self.stage_artifact(&mut artifact)?;
            artifact_paths.push(artifact);
        }
        let policy = policies.iter().min_by_key(|p| p.priority).unwrap();
        let platform_tags = self.resolve_platform_tags(policy);

        let wheel_path = self.write_bin_wheel(
            python_interpreter,
            &artifact_paths,
            &platform_tags,
            &ext_libs,
            sbom_data,
            &result.out_dirs,
        )?;
        eprintln!("📦 Built wheel to {}", wheel_path.display());
        wheels.push((wheel_path, "py3".to_string()));

        Ok(wheels)
    }

    /// Builds a wheel that contains a binary
    ///
    /// Runs [auditwheel_rs()] if not deactivated
    pub fn build_bin_wheels(
        &self,
        interpreters: &[PythonInterpreter],
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        for python_interpreter in interpreters {
            wheels.extend(self.build_bin_wheel(Some(python_interpreter), sbom_data)?);
        }
        Ok(wheels)
    }
}
