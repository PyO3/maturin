use crate::auditwheel::{AuditedArtifact, PlatformTag, Policy};
use crate::binding_generator::{
    BinBindingGenerator, BindingGenerator, CffiBindingGenerator, Pyo3BindingGenerator,
    UniFfiBindingGenerator, generate_binding,
};
use crate::compile::warn_missing_py_init;
use crate::module_writer::{ModuleWriter, WheelWriter, add_data, write_pth};
use crate::pgo::{PgoContext, PgoPhase};
use crate::sbom::SbomData;
use crate::source_distribution::source_distribution;
use crate::target::validate_wheel_filename_for_pypi;
use crate::util::zip_mtime;
use crate::{
    BridgeModel, BuildArtifact, BuildContext, BuiltWheelMetadata, PythonInterpreter, StableAbi,
    StableAbiKind, StableAbiVersion, VirtualWriter, compile, pyproject_toml::Format,
};
use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::CrateType;
use fs_err as fs;
use ignore::overrides::{Override, OverrideBuilder};
use itertools::Itertools;
use normpath::PathExt;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tracing::instrument;

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

    /// Checks which kind of bindings we have (pyo3 or cffi or bin) and calls the
    /// correct builder.
    #[instrument(skip_all)]
    pub fn build_wheels(&self) -> Result<Vec<BuiltWheelMetadata>> {
        if let Some(pgo_command) = &self.context.artifact.pgo_command {
            let needs_per_interpreter_pgo = matches!(
                self.context.project.bridge(),
                BridgeModel::PyO3(crate::PyO3 {
                    stable_abi: None,
                    ..
                })
            );

            eprintln!("🚀 Starting PGO build...");
            PgoContext::find_llvm_profdata()?;

            return if needs_per_interpreter_pgo {
                self.build_wheels_pgo_per_interpreter(pgo_command.clone())
            } else {
                self.build_wheels_pgo_single_pass(pgo_command.clone())
            };
        }
        self.build_wheels_inner()
    }

    /// Single-pass PGO for abi3, cffi, uniffi, and bin builds.
    fn build_wheels_pgo_single_pass(&self, pgo_command: String) -> Result<Vec<BuiltWheelMetadata>> {
        let pgo_ctx = PgoContext::new(pgo_command)?;

        let instrumentation_python = self
            .context
            .python
            .interpreter
            .first()
            .context(
                "PGO builds require a Python interpreter. \
                 Please specify one with `--interpreter`.",
            )?
            .executable
            .clone();

        // Phase 1: Build a single instrumented wheel for training.
        eprintln!("📊 Phase 1/3: Building instrumented wheel...");
        let mut instrumented_ctx = self.clone_context_for_pgo(PgoPhase::Generate(
            pgo_ctx.profdata_dir_path().to_path_buf(),
        ));
        instrumented_ctx.python.interpreter = vec![self.context.python.interpreter[0].clone()];
        let instrumented_out =
            tempfile::TempDir::new().context("Failed to create temp dir for instrumented wheel")?;
        instrumented_ctx.artifact.out = instrumented_out.path().to_path_buf();

        let instrumented_orchestrator = BuildOrchestrator::new(&instrumented_ctx);
        let instrumented_wheels = instrumented_orchestrator.build_wheels_inner()?;

        // Phase 2: Instrumentation
        eprintln!("🔬 Phase 2/3: Running PGO instrumentation...");
        let instrumented_wheel_path = &instrumented_wheels
            .first()
            .context("No instrumented wheel was built")?
            .0;
        pgo_ctx.run_instrumentation(
            &instrumentation_python,
            instrumented_wheel_path,
            self.context,
        )?;
        pgo_ctx.merge_profiles()?;

        // Phase 3: Optimized build
        eprintln!("⚡ Phase 3/3: Building PGO-optimized wheel...");
        let optimized_ctx =
            self.clone_context_for_pgo(PgoPhase::Use(pgo_ctx.merged_profdata_path().to_path_buf()));
        let optimized_orchestrator = BuildOrchestrator::new(&optimized_ctx);
        let wheels = optimized_orchestrator.build_wheels_inner()?;

        eprintln!("🎉 PGO build complete!");
        Ok(wheels)
    }

    /// Per-interpreter PGO for non-abi3 PyO3 builds.
    fn build_wheels_pgo_per_interpreter(
        &self,
        pgo_command: String,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.context.artifact.out)
            .context("Failed to create the target directory for the wheels")?;

        let sbom_data = self.generate_sbom_data()?;
        let mut wheels = Vec::new();

        for (i, python_interpreter) in self.context.python.interpreter.iter().enumerate() {
            eprintln!(
                "📊 [{}/{}] PGO cycle for {} {}.{}...",
                i + 1,
                self.context.python.interpreter.len(),
                python_interpreter.interpreter_kind,
                python_interpreter.major,
                python_interpreter.minor,
            );

            let pgo_ctx = PgoContext::new(pgo_command.clone())?;

            // Phase 1: Build instrumented wheel for this interpreter
            eprintln!("  📊 Phase 1/3: Building instrumented wheel...");
            let mut instrumented_ctx = self.clone_context_for_pgo(PgoPhase::Generate(
                pgo_ctx.profdata_dir_path().to_path_buf(),
            ));
            let instrumented_out = tempfile::TempDir::new()
                .context("Failed to create temp dir for instrumented wheel")?;
            instrumented_ctx.artifact.out = instrumented_out.path().to_path_buf();

            let instrumented_orchestrator = BuildOrchestrator::new(&instrumented_ctx);
            let (instrumented_wheel_path, _) = instrumented_orchestrator
                .build_single_pyo3_wheel(python_interpreter, &sbom_data)?;

            // Phase 2: Run instrumentation with this interpreter
            eprintln!("  🔬 Phase 2/3: Running PGO instrumentation...");
            pgo_ctx.run_instrumentation(
                &python_interpreter.executable,
                &instrumented_wheel_path,
                self.context,
            )?;
            pgo_ctx.merge_profiles()?;

            // Phase 3: Build optimized wheel for this interpreter
            eprintln!("  ⚡ Phase 3/3: Building PGO-optimized wheel...");
            let optimized_ctx = self
                .clone_context_for_pgo(PgoPhase::Use(pgo_ctx.merged_profdata_path().to_path_buf()));
            let optimized_orchestrator = BuildOrchestrator::new(&optimized_ctx);
            let (wheel_path, tag) =
                optimized_orchestrator.build_single_pyo3_wheel(python_interpreter, &sbom_data)?;

            eprintln!(
                "  📦 Built PGO-optimized wheel for {} {}.{}{} to {}",
                python_interpreter.interpreter_kind,
                python_interpreter.major,
                python_interpreter.minor,
                python_interpreter.abiflags,
                wheel_path.display()
            );
            wheels.push((wheel_path, tag));
        }

        // Validate wheel filenames against PyPI platform tag rules if requested
        if self.context.python.pypi_validation {
            for (wheel_path, _) in &wheels {
                let filename = wheel_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| anyhow!("Invalid wheel filename: {:?}", wheel_path))?;

                if let Err(error) = crate::target::validate_wheel_filename_for_pypi(filename) {
                    bail!("PyPI validation failed: {}", error);
                }
            }
        }

        eprintln!("🎉 PGO build complete!");
        Ok(wheels)
    }

    /// Standard wheel build pipeline (no PGO).
    #[instrument(skip_all)]
    pub(crate) fn build_wheels_inner(&self) -> Result<Vec<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.context.artifact.out)
            .context("Failed to create the target directory for the wheels")?;

        // Generate SBOM data once for all wheels (the Rust dependency graph
        // is the same regardless of the target Python interpreter).
        let sbom_data = self.generate_sbom_data()?;

        let interpreters: Vec<_> = self.context.python.interpreter.iter().collect();
        let wheels = match self.context.project.bridge() {
            BridgeModel::Bin(None) => self.build_bin_wheel(None, &sbom_data)?,
            BridgeModel::Bin(Some(..)) => self.build_bin_wheels(&interpreters, &sbom_data)?,
            BridgeModel::PyO3(crate::PyO3 { stable_abi, .. }) => match stable_abi {
                Some(stable_abi) => self.build_stable_abi_wheels(stable_abi, &sbom_data)?,
                None => self.build_pyo3_wheels(&interpreters, &sbom_data)?,
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
    #[instrument(skip_all)]
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
            BridgeModel::PyO3(bindings) | BridgeModel::Bin(Some(bindings)) => {
                let platform = self
                    .context
                    .project
                    .get_platform_tag(&[PlatformTag::Linux])?;
                let interp = &self.context.python.interpreter[0];
                match bindings.stable_abi {
                    Some(stable_abi) => {
                        let wheel_tag = stable_abi.kind.wheel_tag();

                        match stable_abi.version {
                            StableAbiVersion::Version(major, minor) => {
                                vec![format!("cp{major}{minor}-{wheel_tag}-{platform}")]
                            }
                            StableAbiVersion::CurrentPython => {
                                vec![format!(
                                    "cp{major}{minor}-{wheel_tag}-{platform}",
                                    major = interp.major,
                                    minor = interp.minor
                                )]
                            }
                        }
                    }
                    None => {
                        vec![
                            self.context.python.interpreter[0]
                                .get_tag(&self.context.project, &[PlatformTag::Linux])?,
                        ]
                    }
                }
            }
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
    fn get_universal_tag(&self, platform_tags: &[PlatformTag]) -> Result<String> {
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
    #[instrument(skip_all)]
    pub(crate) fn build_stable_abi_wheels(
        &self,
        stable_abi: &StableAbi,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let min_version = stable_abi.version.min_version();
        let stable_abi_interps: Vec<_> = self
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
            .collect();
        let version_specific_abi_interps: Vec<_> = self
            .context
            .python
            .interpreter
            .iter()
            .filter(|interp| !interp.has_stable_api())
            .collect();

        if stable_abi_interps.is_empty() && version_specific_abi_interps.is_empty() {
            let interp_names: Vec<_> = self
                .context
                .python
                .interpreter
                .iter()
                .map(|interp| {
                    format!(
                        "{} {}.{}",
                        interp.interpreter_kind, interp.major, interp.minor
                    )
                })
                .collect();
            if let Some((major, minor)) = min_version {
                bail!(
                    "None of the found Python interpreters ({}) are compatible with the abi3 \
                     minimum version (>= {}.{}). Please install a compatible Python interpreter.",
                    interp_names.join(", "),
                    major,
                    minor,
                );
            } else {
                bail!(
                    "No compatible Python interpreters found for abi3 build. \
                     Found: {}",
                    interp_names.join(", "),
                );
            }
        }

        let mut built_wheels = Vec::new();
        if let Some(first) = stable_abi_interps.first() {
            let (major, minor) = min_version.unwrap_or((first.major as u8, first.minor as u8));
            built_wheels.extend(self.build_pyo3_wheel_stable_abi(
                &stable_abi_interps,
                stable_abi.kind,
                major,
                minor,
                sbom_data,
            )?);
        }
        if !version_specific_abi_interps.is_empty() {
            let interp_names: HashSet<_> = version_specific_abi_interps
                .iter()
                .map(|interp| interp.to_string())
                .collect();
            eprintln!(
                "⚠️ Warning: {} does not yet support {} so the build artifacts will be version-specific.",
                stable_abi.kind,
                interp_names.iter().join(", ")
            );
            built_wheels.extend(self.build_pyo3_wheels(&version_specific_abi_interps, sbom_data)?);
        }
        Ok(built_wheels)
    }

    /// The internal wheel-writing loop. Handles metadata generation, file compression,
    /// and writing the final .whl archive to the output directory.
    #[allow(clippy::too_many_arguments, clippy::needless_lifetimes)]
    fn write_wheel<'b, F>(
        &'b self,
        tag: &str,
        audited: &[AuditedArtifact],
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
        self.context.add_external_libs(&mut writer, audited)?;

        let temp_dir = writer.temp_dir()?;
        let mut generator = make_generator(temp_dir)?;
        generate_binding(
            &mut writer,
            generator.as_mut(),
            self.context,
            audited,
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
    #[instrument(skip_all)]
    pub(crate) fn build_pyo3_wheel_stable_abi(
        &self,
        interpreters: &[&PythonInterpreter],
        stable_abi_kind: StableAbiKind,
        major: u8,
        min_minor: u8,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        let python_interpreter = interpreters.first().copied();
        let (artifact, out_dirs) = self.compile_cdylib(
            python_interpreter,
            Some(&self.context.project.project_layout.extension_name),
        )?;
        let audit_result = self.context.auditwheel(
            &artifact,
            &self.context.python.platform_tag,
            python_interpreter,
        )?;
        let platform_tags = self.resolve_platform_tags(&audit_result.policy);

        let platform = self.context.project.get_platform_tag(&platform_tags)?;
        let abi_tag = stable_abi_kind.wheel_tag();
        let tag = format!("cp{major}{min_minor}-{abi_tag}-{platform}");

        let audited = [AuditedArtifact {
            artifact,
            external_libs: audit_result.external_libs,
            arch_requirements: audit_result.arch_requirements,
        }];
        let wheel_path = self.write_wheel(
            &tag,
            &audited,
            |temp_dir| {
                Ok(Box::new(
                    Pyo3BindingGenerator::new(Some(stable_abi_kind), python_interpreter, temp_dir)
                        .context("Failed to initialize PyO3 binding generator")?,
                ))
            },
            sbom_data,
            &out_dirs,
        )?;

        eprintln!(
            "📦 Built wheel for {stable_abi_kind} Python ≥ {}.{} to {}",
            major,
            min_minor,
            wheel_path.display()
        );
        wheels.push((wheel_path, format!("cp{major}{min_minor}")));

        Ok(wheels)
    }

    /// Writes a PyO3 wheel for a specific Python interpreter.
    fn write_pyo3_wheel(
        &self,
        python_interpreter: &PythonInterpreter,
        audited: &[AuditedArtifact],
        platform_tags: &[PlatformTag],
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<PathBuf> {
        let tag = python_interpreter.get_tag(&self.context.project, platform_tags)?;

        self.write_wheel(
            &tag,
            audited,
            |temp_dir| {
                Ok(Box::new(
                    Pyo3BindingGenerator::new(None, Some(python_interpreter), temp_dir)
                        .context("Failed to initialize PyO3 binding generator")?,
                ))
            },
            sbom_data,
            out_dirs,
        )
    }

    /// Compile, audit, and write a single PyO3 wheel for one interpreter.
    #[instrument(skip_all)]
    pub(crate) fn build_single_pyo3_wheel(
        &self,
        python_interpreter: &PythonInterpreter,
        sbom_data: &Option<SbomData>,
    ) -> Result<BuiltWheelMetadata> {
        let (artifact, out_dirs) = self.compile_cdylib(
            Some(python_interpreter),
            Some(&self.context.project.project_layout.extension_name),
        )?;
        let audit_result = self.context.auditwheel(
            &artifact,
            &self.context.python.platform_tag,
            Some(python_interpreter),
        )?;
        let platform_tags = self.resolve_platform_tags(&audit_result.policy);
        let audited = [AuditedArtifact {
            artifact,
            external_libs: audit_result.external_libs,
            arch_requirements: audit_result.arch_requirements,
        }];
        let wheel_path = self.write_pyo3_wheel(
            python_interpreter,
            &audited,
            &platform_tags,
            sbom_data,
            &out_dirs,
        )?;
        let tag = format!("cp{}{}", python_interpreter.major, python_interpreter.minor);
        Ok((wheel_path, tag))
    }

    /// Builds wheels for a pyo3 extension for all given python versions.
    #[instrument(skip_all)]
    pub(crate) fn build_pyo3_wheels(
        &self,
        interpreters: &[&PythonInterpreter],
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        for &python_interpreter in interpreters {
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
    #[instrument(skip_all)]
    pub(crate) fn compile_cdylib(
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

    /// Compiles a cdylib and builds a wheel for it.
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
        let audit_result =
            self.context
                .auditwheel(&artifact, &self.context.python.platform_tag, None)?;
        let platform_tags = self.resolve_platform_tags(&audit_result.policy);
        let tag = self.get_universal_tag(&platform_tags)?;
        let audited = [AuditedArtifact {
            artifact,
            external_libs: audit_result.external_libs,
            arch_requirements: audit_result.arch_requirements,
        }];
        let wheel_path = self.write_wheel(&tag, &audited, make_generator, sbom_data, &out_dirs)?;
        Ok((wheel_path, out_dirs))
    }

    /// Builds a wheel with cffi bindings
    #[instrument(skip_all)]
    fn build_cffi_wheel(&self, sbom_data: &Option<SbomData>) -> Result<Vec<BuiltWheelMetadata>> {
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
    #[instrument(skip_all)]
    fn build_uniffi_wheel(&self, sbom_data: &Option<SbomData>) -> Result<Vec<BuiltWheelMetadata>> {
        let (wheel_path, _) = self.build_cdylib_wheel(
            |_temp_dir| Ok(Box::new(UniFfiBindingGenerator::default())),
            sbom_data,
        )?;

        eprintln!("📦 Built wheel to {}", wheel_path.display());
        Ok(vec![(wheel_path, "py3".to_string())])
    }

    /// Internal implementation for writing a binary wheel.
    fn write_bin_wheel(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        audited: &[AuditedArtifact],
        platform_tags: &[PlatformTag],
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

        self.context.add_external_libs(&mut writer, audited)?;

        let mut generator = BinBindingGenerator::new(&mut metadata24);
        generate_binding(&mut writer, &mut generator, self.context, audited, out_dirs)
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
    #[instrument(skip_all)]
    fn build_bin_wheel(
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
        let mut audited_artifacts = Vec::new();
        for artifact in result.artifacts {
            let mut artifact = artifact
                .get(&CrateType::Bin)
                .cloned()
                .ok_or_else(|| anyhow!("Cargo didn't build a binary"))?;

            let audit_result =
                self.context
                    .auditwheel(&artifact, &self.context.python.platform_tag, None)?;
            policies.push(audit_result.policy);

            self.context.stage_artifact(&mut artifact)?;
            audited_artifacts.push(AuditedArtifact {
                artifact,
                external_libs: audit_result.external_libs,
                arch_requirements: audit_result.arch_requirements,
            });
        }
        let policy = policies.iter().min_by_key(|p| p.priority).unwrap();
        let platform_tags = self.resolve_platform_tags(policy);

        let wheel_path = self.write_bin_wheel(
            python_interpreter,
            &audited_artifacts,
            &platform_tags,
            sbom_data,
            &result.out_dirs,
        )?;
        eprintln!("📦 Built wheel to {}", wheel_path.display());
        wheels.push((wheel_path, "py3".to_string()));

        Ok(wheels)
    }

    /// Builds wheels for a binary project for all given python versions.
    #[instrument(skip_all)]
    pub(crate) fn build_bin_wheels(
        &self,
        interpreters: &[&PythonInterpreter],
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        for &python_interpreter in interpreters {
            wheels.extend(self.build_bin_wheel(Some(python_interpreter), sbom_data)?);
        }
        Ok(wheels)
    }

    /// Generate Rust SBOMs once from the build context.
    pub(crate) fn generate_sbom_data(&self) -> Result<Option<SbomData>> {
        SbomData::generate(self.context)
    }

    /// Writes SBOMs into the wheel via the given writer.
    pub(crate) fn write_sboms(
        &self,
        sbom_data: Option<&SbomData>,
        writer: &mut impl ModuleWriter,
        dist_info_dir: &Path,
    ) -> Result<()> {
        SbomData::write(sbom_data, self.context, writer, dist_info_dir)
    }
}
