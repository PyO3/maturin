use crate::auditwheel::{PlatformTag, Policy};
use crate::binding_generator::{
    BinBindingGenerator, BindingGenerator, CffiBindingGenerator, Pyo3BindingGenerator,
    UniFfiBindingGenerator, generate_binding,
};
use crate::bridge::Abi3Version;
use crate::compile::warn_missing_py_init;
use crate::module_writer::{ModuleWriter, WheelWriter, add_data, write_pth};
use crate::pgo::{PgoContext, PgoPhase};
use crate::sbom::{SbomData, resolve_sbom_include};
use crate::source_distribution::source_distribution;
use crate::target::validate_wheel_filename_for_pypi;
use crate::util::zip_mtime;
use crate::{
    BridgeModel, BuildArtifact, BuildContext, BuiltWheelMetadata, PythonInterpreter, VirtualWriter,
    compile, pyproject_toml::Format,
};
use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::CrateType;
use fs_err as fs;
use ignore::overrides::{Override, OverrideBuilder};
use itertools::Itertools;
use lddtree::Library;
use normpath::PathExt;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tracing::instrument;

#[cfg(feature = "sbom")]
use cargo_cyclonedx::config::SbomConfig as CyclonedxConfig;
#[cfg(feature = "sbom")]
use cargo_cyclonedx::generator::SbomGenerator;

/// Orchestrates the build process using the data provided by [BuildContext].
///
/// This struct decouples the high-level build logic (the "verbs") from the
/// partitioned data contexts (the "nouns").
pub struct BuildOrchestrator<'a> {
    context: &'a BuildContext,
}

impl<'a> BuildOrchestrator<'a> {
    /// Creates a new orchestrator for the given build context.
    pub fn new(context: &'a BuildContext) -> Self {
        Self { context }
    }

    /// Returns the underlying build context.
    pub fn context(&self) -> &BuildContext {
        self.context
    }

    /// Checks which kind of bindings we have (pyo3/rust-cypthon or cffi or bin) and calls the
    /// correct builder.
    #[instrument(skip_all)]
    pub fn build_wheels(&self) -> Result<Vec<BuiltWheelMetadata>> {
        if let Some(pgo_command) = &self.context.artifact.pgo_command {
            let pgo_ctx = PgoContext::new(pgo_command.clone())?;
            return pgo_ctx.build_wheels_pgo(self);
        }
        self.build_wheels_inner()
    }

    /// Standard wheel build pipeline (no PGO).
    pub(crate) fn build_wheels_inner(&self) -> Result<Vec<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.context.artifact.out)
            .context("Failed to create the target directory for the wheels")?;

        // Generate SBOM data once for all wheels (the Rust dependency graph
        // is the same regardless of the target Python interpreter).
        let sbom_data = self.generate_sbom_data()?;

        let wheels = match self.context.project.bridge() {
            BridgeModel::Bin(None) => self.build_bin_wheel(None, &sbom_data)?,
            BridgeModel::Bin(Some(..)) => {
                self.build_bin_wheels(&self.context.python.interpreter, &sbom_data)?
            }
            BridgeModel::PyO3(crate::PyO3 { abi3, .. }) => match abi3 {
                Some(Abi3Version::Version(major, minor)) => {
                    self.build_abi3_wheels(Some((*major, *minor)), &sbom_data)?
                }
                Some(Abi3Version::CurrentPython) => self.build_abi3_wheels(None, &sbom_data)?,
                None => self.build_pyo3_wheels(&self.context.python.interpreter, &sbom_data)?,
            },
            BridgeModel::Cffi => self.build_cffi_wheel(&sbom_data)?,
            BridgeModel::UniFfi => self.build_uniffi_wheel(&sbom_data)?,
        };

        // Validate wheel filenames against PyPI platform tag rules if requested
        if self.context.python.pypi_validation {
            for wheel in &wheels {
                let filename = wheel
                    .0
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| anyhow!("Invalid wheel filename: {:?}", wheel.0))?;

                if let Err(error) = validate_wheel_filename_for_pypi(filename) {
                    bail!("PyPI validation failed: {}", error);
                }
            }
        }

        Ok(wheels)
    }

    /// Clone the context with PGO disabled (to prevent recursion) and
    /// the given PGO phase set.
    pub(crate) fn clone_context_for_pgo(&self, phase: PgoPhase) -> BuildContext {
        let mut ctx = self.context.clone();
        ctx.artifact.pgo_command = None;
        ctx.artifact.pgo_phase = Some(phase);
        ctx
    }

    /// Builds a source distribution and returns the same metadata as [BuildOrchestrator::build_wheels]
    pub fn build_source_distribution(&self) -> Result<Option<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.context.artifact.out)
            .context("Failed to create the target directory for the source distribution")?;

        match self.context.project.pyproject_toml.as_ref() {
            Some(pyproject) => {
                let sdist_path = source_distribution(
                    &self.context.project,
                    &self.context.artifact,
                    pyproject,
                    self.excludes(Format::Sdist)?,
                )
                .context("Failed to build source distribution")?;
                Ok(Some((sdist_path, "source".to_string())))
            }
            None => Ok(None),
        }
    }

    /// Return the tags of the wheel that this build context builds.
    pub fn tags_from_bridge(&self) -> Result<Vec<String>> {
        let tags = match self.context.project.bridge() {
            BridgeModel::PyO3(bindings) | BridgeModel::Bin(Some(bindings)) => match bindings.abi3 {
                Some(Abi3Version::Version(major, minor)) => {
                    let platform = self
                        .context
                        .project
                        .get_platform_tag(&[PlatformTag::Linux])?;
                    vec![format!("cp{major}{minor}-abi3-{platform}")]
                }
                Some(Abi3Version::CurrentPython) => {
                    let interp = &self.context.python.interpreter[0];
                    let platform = self
                        .context
                        .project
                        .get_platform_tag(&[PlatformTag::Linux])?;
                    vec![format!(
                        "cp{major}{minor}-abi3-{platform}",
                        major = interp.major,
                        minor = interp.minor
                    )]
                }
                None => {
                    vec![
                        self.context.python.interpreter[0]
                            .get_tag(&self.context.project, &[PlatformTag::Linux])?,
                    ]
                }
            },
            BridgeModel::Bin(None) | BridgeModel::Cffi | BridgeModel::UniFfi => {
                vec![self.get_universal_tag(&[PlatformTag::Linux])?]
            }
        };
        Ok(tags)
    }

    fn add_pth(&self, writer: &mut VirtualWriter<WheelWriter>) -> Result<()> {
        if self.context.project.editable {
            write_pth(
                writer,
                &self.context.project.project_layout,
                &self.context.project.metadata24,
            )?;
        }
        Ok(())
    }

    fn excludes(&self, format: Format) -> Result<Override> {
        let project_dir = match self.context.project.pyproject_toml_path.normalize() {
            Ok(pyproject_toml_path) => pyproject_toml_path.into_path_buf(),
            Err(_) => self
                .context
                .project
                .manifest_path
                .normalize()?
                .into_path_buf(),
        };
        let mut excludes = OverrideBuilder::new(project_dir.parent().unwrap());
        if let Some(pyproject) = self.context.project.pyproject_toml.as_ref()
            && let Some(glob_patterns) = &pyproject.exclude()
        {
            for glob in glob_patterns
                .iter()
                .filter_map(|glob_pattern| glob_pattern.targets(format))
            {
                excludes.add(glob)?;
            }
        }
        // Ignore sdist output files so that we don't include them in the sdist
        if matches!(format, Format::Sdist) {
            let glob_pattern = format!(
                "{}{}{}-*.tar.gz",
                self.context.artifact.out.display(),
                std::path::MAIN_SEPARATOR,
                &self.context.project.metadata24.get_distribution_escaped(),
            );
            excludes.add(&glob_pattern)?;
        }
        Ok(excludes.build()?)
    }

    /// Returns the platform tag without python version (e.g. `py3-none-manylinux_2_17_x86_64`)
    pub fn get_universal_tag(&self, platform_tags: &[PlatformTag]) -> Result<String> {
        let platform = self.context.project.get_platform_tag(platform_tags)?;
        Ok(format!("py3-none-{platform}"))
    }

    /// Returns user-specified platform tags, or falls back to the auditwheel
    /// policy tag when no explicit tags were provided.
    fn resolve_platform_tags(&self, policy: &Policy) -> Vec<PlatformTag> {
        if self.context.python.platform_tag.is_empty() {
            vec![policy.platform_tag()]
        } else {
            self.context.python.platform_tag.clone()
        }
    }

    /// Split interpreters into abi3-capable and non-abi3 groups, build the
    /// appropriate wheel type for each group, and return all built wheels.
    pub(crate) fn build_abi3_wheels(
        &self,
        min_version: Option<(u8, u8)>,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let abi3_interps: Vec<_> = self
            .context
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
            .context
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

    #[allow(clippy::too_many_arguments, clippy::needless_lifetimes)]
    fn write_wheel<'b, F>(
        &'b self,
        tag: &str,
        artifacts: &[&BuildArtifact],
        ext_libs: &[Vec<Library>],
        make_generator: F,
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<PathBuf>
    where
        F: FnOnce(Rc<tempfile::TempDir>) -> Result<Box<dyn BindingGenerator + 'b>>,
    {
        let file_options = self
            .context
            .artifact
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let writer = WheelWriter::new(
            tag,
            &self.context.artifact.out,
            &self.context.project.metadata24,
            file_options,
        )?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);
        self.context
            .add_external_libs(&mut writer, artifacts, ext_libs)?;

        let temp_dir = writer.temp_dir()?;
        let mut generator = make_generator(temp_dir)?;
        generate_binding(
            &mut writer,
            generator.as_mut(),
            self.context,
            artifacts,
            out_dirs,
        )
        .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &self.context.project.metadata24,
            self.context.project.project_layout.data.as_deref(),
        )?;

        self.write_sboms(
            sbom_data.as_ref(),
            &mut writer,
            &self.context.project.metadata24.get_dist_info_dir(),
        )?;

        let tags = [tag.to_string()];
        let wheel_path = writer.finish(
            &self.context.project.metadata24,
            &self.context.project.project_layout.project_root,
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
        let python_interpreter = interpreters.first();
        let (artifact, out_dirs) = self.compile_cdylib(
            python_interpreter,
            Some(&self.context.project.project_layout.extension_name),
        )?;
        let (policy, external_libs) = self.context.auditwheel(
            &artifact,
            &self.context.python.platform_tag,
            python_interpreter,
        )?;
        let platform_tags = self.resolve_platform_tags(&policy);

        let platform = self.context.project.get_platform_tag(&platform_tags)?;
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
        let tag = python_interpreter.get_tag(&self.context.project, platform_tags)?;

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
    pub(crate) fn build_single_pyo3_wheel(
        &self,
        python_interpreter: &PythonInterpreter,
        sbom_data: &Option<SbomData>,
    ) -> Result<BuiltWheelMetadata> {
        let (artifact, out_dirs) = self.compile_cdylib(
            Some(python_interpreter),
            Some(&self.context.project.project_layout.extension_name),
        )?;
        let (policy, external_libs) = self.context.auditwheel(
            &artifact,
            &self.context.python.platform_tag,
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
    pub fn compile_cdylib(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        extension_name: Option<&str>,
    ) -> Result<(BuildArtifact, HashMap<String, PathBuf>)> {
        let result = compile(
            self.context,
            python_interpreter,
            &self.context.project.compile_targets,
        )
        .context("Failed to build a native library through cargo")?;
        let error_msg = "Cargo didn't build a cdylib. Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?";
        let artifacts = result.artifacts.first().context(error_msg)?;

        let mut artifact = artifacts
            .get(&CrateType::CDyLib)
            .cloned()
            .ok_or_else(|| anyhow!(error_msg,))?;

        self.context.stage_artifact(&mut artifact)?;

        if let Some(extension_name) = extension_name {
            let _ = warn_missing_py_init(&artifact.path, extension_name);
        }
        Ok((artifact, result.out_dirs))
    }

    #[allow(clippy::needless_lifetimes)]
    fn build_cdylib_wheel<'b, F>(
        &'b self,
        make_generator: F,
        sbom_data: &Option<SbomData>,
    ) -> Result<(PathBuf, HashMap<String, PathBuf>)>
    where
        F: FnOnce(Rc<tempfile::TempDir>) -> Result<Box<dyn BindingGenerator + 'b>>,
    {
        let (artifact, out_dirs) = self.compile_cdylib(None, None)?;
        let (policy, external_libs) =
            self.context
                .auditwheel(&artifact, &self.context.python.platform_tag, None)?;
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
        let interpreter = self.context.python.interpreter.first().ok_or_else(|| {
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

        if !self
            .context
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
        if !self.context.project.metadata24.scripts.is_empty() {
            bail!("Defining scripts and working with a binary doesn't mix well");
        }

        if self.context.project.target.is_wasi() {
            eprintln!("⚠️  Warning: wasi support is experimental");
            if !self.context.project.metadata24.entry_points.is_empty() {
                bail!("You can't define entrypoints yourself for a binary project");
            }

            if self.context.project.project_layout.python_module.is_some() {
                bail!("Sorry, adding python code to a wasm binary is currently not supported")
            }
        }

        let tag = match (self.context.project.bridge(), python_interpreter) {
            (BridgeModel::Bin(None), _) => self.get_universal_tag(platform_tags)?,
            (BridgeModel::Bin(Some(..)), Some(python_interpreter)) => {
                python_interpreter.get_tag(&self.context.project, platform_tags)?
            }
            _ => unreachable!(),
        };

        let mut metadata24 = self.context.project.metadata24.clone();
        let file_options = self
            .context
            .artifact
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let writer = WheelWriter::new(&tag, &self.context.artifact.out, &metadata24, file_options)?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);

        let artifact_refs: Vec<&BuildArtifact> = artifacts.iter().collect();
        self.context
            .add_external_libs(&mut writer, &artifact_refs, ext_libs)?;

        let mut generator = BinBindingGenerator::new(&mut metadata24);
        generate_binding(
            &mut writer,
            &mut generator,
            self.context,
            artifacts,
            out_dirs,
        )
        .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &metadata24,
            self.context.project.project_layout.data.as_deref(),
        )?;

        self.write_sboms(
            sbom_data.as_ref(),
            &mut writer,
            &metadata24.get_dist_info_dir(),
        )?;

        let tags = [tag];
        let wheel_path = writer.finish(
            &metadata24,
            &self.context.project.project_layout.project_root,
            &tags,
        )?;
        Ok(wheel_path)
    }

    /// Builds a wheel that contains a binary
    pub fn build_bin_wheel(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        let result = compile(
            self.context,
            python_interpreter,
            &self.context.project.compile_targets,
        )
        .context("Failed to build a native library through cargo")?;
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
                self.context
                    .auditwheel(&artifact, &self.context.python.platform_tag, None)?;
            policies.push(policy);
            ext_libs.push(external_libs);

            self.context.stage_artifact(&mut artifact)?;
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

    /// Builds wheels for a binary project for all given python versions.
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

    /// Generate Rust SBOMs once from the build context.
    pub(crate) fn generate_sbom_data(&self) -> Result<Option<SbomData>> {
        let sbom_config = self.context.artifact.sbom.as_ref();

        // Check if Rust SBOM generation is explicitly disabled
        let rust_sbom_enabled = sbom_config.and_then(|c| c.rust).unwrap_or(true);

        #[cfg(feature = "sbom")]
        {
            if !rust_sbom_enabled {
                return Ok(Some(SbomData {
                    rust_sboms: Vec::new(),
                }));
            }

            let config = CyclonedxConfig {
                target: Some(cargo_cyclonedx::config::Target::AllTargets),
                ..CyclonedxConfig::empty_config()
            };
            // cargo-cyclonedx depends on cargo_metadata 0.18, while maturin uses
            // cargo_metadata 0.23. The Metadata structs are incompatible at the
            // type level but share the same JSON representation, so we bridge
            // them via a serde round-trip.
            let json = serde_json::to_value(&self.context.project.cargo_metadata)?;
            let metadata = serde_json::from_value(json)
                .context("Failed to convert cargo metadata for SBOM generation")?;
            let sboms = SbomGenerator::create_sboms(metadata, &config)
                .map_err(|e| anyhow::anyhow!("Failed to generate Rust SBOM: {}", e))?;

            let mut rust_sboms = Vec::new();
            for sbom in sboms {
                // Only keep the SBOM for the crate being built into a wheel.
                // Each member's SBOM already contains the full transitive
                // dependency graph, so filtering is safe.
                if sbom.package_name != self.context.project.crate_name {
                    continue;
                }
                let mut buf = Vec::new();
                sbom.bom
                    .output_as_json_v1_5(&mut buf)
                    .map_err(|e| anyhow::anyhow!("Failed to serialize SBOM: {}", e))?;
                rust_sboms.push((sbom.package_name, buf));
            }

            Ok(Some(SbomData { rust_sboms }))
        }

        #[cfg(not(feature = "sbom"))]
        {
            let _ = rust_sbom_enabled;
            Ok(None)
        }
    }

    /// Writes SBOMs into the wheel via the given writer.
    pub(crate) fn write_sboms(
        &self,
        sbom_data: Option<&SbomData>,
        writer: &mut impl ModuleWriter,
        dist_info_dir: &Path,
    ) -> Result<()> {
        let sbom_config = self.context.artifact.sbom.as_ref();

        // 1. Write pre-generated Rust SBOMs
        if let Some(data) = sbom_data {
            for (package_name, json_bytes) in &data.rust_sboms {
                let target = dist_info_dir.join(format!("sboms/{package_name}.cyclonedx.json"));
                writer.add_bytes(&target, None, json_bytes.clone(), false)?;
            }
        }

        // 2. Include additional SBOM files (only when explicitly configured)
        if let Some(include) = sbom_config.and_then(|c| c.include.as_ref()) {
            // Canonicalize project root once and enforce all includes stay within it.
            let project_root = self
                .context
                .project
                .project_layout
                .project_root
                .canonicalize()
                .context("Failed to canonicalize project root for SBOM includes")?;

            let mut seen_filenames = HashSet::new();
            for path in include {
                let resolved_path = resolve_sbom_include(path, &project_root)?;

                let filename = resolved_path.file_name().context("Invalid SBOM path")?;
                if !seen_filenames.insert(filename.to_os_string()) {
                    anyhow::bail!(
                        "Duplicate SBOM filename '{}' from include path '{}'. \
                         Multiple includes must have unique filenames.",
                        filename.to_string_lossy(),
                        path.display()
                    );
                }
                let target = dist_info_dir.join("sboms").join(filename);
                writer.add_file(&target, &resolved_path, false)?;
            }
        }

        Ok(())
    }
}
