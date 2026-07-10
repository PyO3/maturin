use crate::auditwheel::{AuditWheelMode, AuditedArtifact, PlatformTag, Policy};
use crate::binding_generator::{
    BinBindingGenerator, BindingGenerator, CffiBindingGenerator, Pyo3BindingGenerator,
    UniFfiBindingGenerator, generate_binding,
};
use crate::build_context::finalize_staged_artifacts;
use crate::compile::{missing_cdylib_error, missing_cdylib_message, warn_missing_py_init};
use crate::module_writer::{WheelWriter, add_data, write_pth};
use crate::pgo::{PgoContext, PgoPhase};
use crate::sbom::SbomData;
use crate::source_distribution::source_distribution;
use crate::target::{WheelTag, validate_wheel_filename_for_pypi};
use crate::ui;
use crate::util::zip_mtime;
use crate::{
    BridgeModel, BuildArtifact, BuildContext, BuiltArtifactTag, BuiltWheel, Metadata24,
    PythonInterpreter, StableAbi, VirtualWriter, compile, pyproject_toml::Format,
};
use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::CrateType;
use fs_err as fs;
use ignore::overrides::{Override, OverrideBuilder};
use itertools::Itertools;
use normpath::PathExt;
use pyo3_introspection::{introspect_cdylib, module_stub_files};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
    pub fn build_wheels(&self) -> Result<Vec<BuiltWheel>> {
        if self.context.artifact.pgo_command.is_some() {
            ui::status!("🚀 Starting PGO build...");
            PgoContext::find_llvm_profdata()?;
        }
        let wheels = self.build_wheels_inner()?;
        if self.context.artifact.pgo_command.is_some() {
            ui::status!("🎉 PGO build complete!");
        }
        Ok(wheels)
    }

    /// Wrapper around a single wheel build that runs a PGO cycle if needed
    fn build_single_unit<F>(
        &self,
        instrumentation_python: Option<&PythonInterpreter>,
        build: F,
    ) -> Result<Vec<BuiltWheel>>
    where
        F: for<'ctx> Fn(&BuildOrchestrator<'ctx>) -> Result<Vec<BuiltWheel>>,
    {
        let Some(pgo_command) = self.context.artifact.pgo_command.clone() else {
            return build(self);
        };
        let instrumentation_python = instrumentation_python.context(
            "PGO builds require a Python interpreter. Please specify one with `--interpreter`.",
        )?;

        self.run_pgo_cycle(
            pgo_command,
            instrumentation_python,
            "",
            |instrumented| {
                let wheels = build(instrumented)?;
                Ok(wheels
                    .first()
                    .context("No instrumented wheel was built")?
                    .path
                    .clone())
            },
            |optimized| build(optimized),
        )
    }

    fn pgo_instrumentation_interpreter(&self) -> Result<Option<&PythonInterpreter>> {
        if self.context.artifact.pgo_command.is_none() {
            return Ok(None);
        }
        self.context
            .python
            .interpreter
            .first()
            .context(
                "PGO builds require a Python interpreter. Please specify one with `--interpreter`.",
            )
            .map(Some)
    }

    /// Runs the shared PGO cycle: instrumented build, instrumentation, then optimized build.
    ///
    /// `message_prefix` preserves user-visible indentation for per-interpreter PGO output.
    fn run_pgo_cycle<T, BuildInstrumented, BuildOptimized>(
        &self,
        pgo_command: String,
        instrumentation_python: &PythonInterpreter,
        message_prefix: &str,
        build_instrumented_wheel: BuildInstrumented,
        build_optimized_wheels: BuildOptimized,
    ) -> Result<T>
    where
        BuildInstrumented: for<'ctx> FnOnce(&BuildOrchestrator<'ctx>) -> Result<PathBuf>,
        BuildOptimized: for<'ctx> FnOnce(&BuildOrchestrator<'ctx>) -> Result<T>,
    {
        let pgo_ctx = PgoContext::new(pgo_command)?;

        ui::status!("{message_prefix}📊 Phase 1/3: Building instrumented wheel...");
        let mut instrumented_ctx = self.clone_context_for_pgo(PgoPhase::Generate(
            pgo_ctx.profdata_dir_path().to_path_buf(),
        ));
        let instrumented_out =
            tempfile::TempDir::new().context("Failed to create temp dir for instrumented wheel")?;
        instrumented_ctx.artifact.out = instrumented_out.path().to_path_buf();

        let instrumented_orchestrator = BuildOrchestrator::new(&instrumented_ctx);
        let instrumented_wheel_path = build_instrumented_wheel(&instrumented_orchestrator)?;

        ui::status!("{message_prefix}🔬 Phase 2/3: Running PGO instrumentation...");
        pgo_ctx.run_instrumentation(
            instrumentation_python,
            &instrumented_wheel_path,
            self.context,
        )?;
        pgo_ctx.merge_profiles()?;

        ui::status!("{message_prefix}⚡ Phase 3/3: Building PGO-optimized wheel...");
        let optimized_ctx =
            self.clone_context_for_pgo(PgoPhase::Use(pgo_ctx.merged_profdata_path().to_path_buf()));
        let optimized_orchestrator = BuildOrchestrator::new(&optimized_ctx);
        build_optimized_wheels(&optimized_orchestrator)
    }

    /// Standard wheel build pipeline (no PGO).
    #[instrument(skip_all)]
    pub(crate) fn build_wheels_inner(&self) -> Result<Vec<BuiltWheel>> {
        fs::create_dir_all(&self.context.artifact.out)
            .context("Failed to create the target directory for the wheels")?;

        // Generate SBOM data once for all wheels (the Rust dependency graph
        // is the same regardless of the target Python interpreter).
        let sbom_data = SbomData::generate(self.context)?;

        let interpreters: Vec<_> = self.context.python.interpreter.iter().collect();
        let wheels = match self.context.project.bridge() {
            BridgeModel::Bin(None) => self
                .build_single_unit(self.pgo_instrumentation_interpreter()?, |orchestrator| {
                    orchestrator.build_bin_wheel(None, &sbom_data)
                })?,
            BridgeModel::Bin(Some(..)) => self.build_bin_wheels(&interpreters, &sbom_data)?,
            BridgeModel::PyO3(crate::PyO3 { stable_abi, .. }) => match stable_abi {
                Some(stable_abi) => self.build_stable_abi_wheels(stable_abi, &sbom_data)?,
                None => self.build_pyo3_wheels(&interpreters, &sbom_data)?,
            },
            BridgeModel::Cffi => self
                .build_single_unit(self.context.python.interpreter.first(), |orchestrator| {
                    orchestrator.build_cffi_wheel(&sbom_data)
                })?,
            BridgeModel::UniFfi => self
                .build_single_unit(self.pgo_instrumentation_interpreter()?, |orchestrator| {
                    orchestrator.build_uniffi_wheel(&sbom_data)
                })?,
        };

        self.validate_wheels_for_pypi(&wheels)?;

        Ok(wheels)
    }

    /// Validates built wheel filenames against PyPI platform tag rules when
    /// `--compatibility pypi` validation was requested.
    fn validate_wheels_for_pypi(&self, wheels: &[BuiltWheel]) -> Result<()> {
        if self.context.python.pypi_validation {
            for wheel in wheels {
                let filename = wheel
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| anyhow!("Invalid wheel filename: {:?}", wheel.path))?;

                if let Err(error) = validate_wheel_filename_for_pypi(filename) {
                    bail!("PyPI validation failed: {}", error);
                }
            }
        }
        Ok(())
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
    pub fn build_source_distribution(&self) -> Result<Option<BuiltWheel>> {
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
                Ok(Some(BuiltWheel {
                    path: sdist_path,
                    tag: BuiltArtifactTag::Source,
                }))
            }
            None => Ok(None),
        }
    }

    /// Return the tags of the wheel that this build context builds.
    pub fn tags_from_bridge(&self) -> Result<Vec<WheelTag>> {
        let bridge = self.context.project.bridge();
        let tags = match bridge {
            BridgeModel::PyO3(bindings) | BridgeModel::Bin(Some(bindings)) => {
                let platform = self
                    .context
                    .project
                    .get_platform_tag(&[PlatformTag::Linux])?;
                let interp = self
                    .context
                    .python
                    .interpreter
                    .first()
                    .context("no python interpreter resolved for tag computation")?;
                match bindings.stable_abi {
                    Some(stable_abi) => {
                        let min_version = stable_abi.version.min_version();
                        let (stable_abi_interps, version_specific_interps): (Vec<_>, Vec<_>) = self
                            .context
                            .python
                            .interpreter
                            .iter()
                            .partition(|interp| bridge.is_stable_abi_for_interpreter(interp));
                        let abi_tag = stable_abi.kind.wheel_tag();
                        let stable_abi_tag = stable_abi_interps.first().map(|interp| {
                            let (major, minor) =
                                min_version.unwrap_or((interp.major as u8, interp.minor as u8));
                            WheelTag::new(format!("cp{major}{minor}"), abi_tag, platform.clone())
                        });
                        // Some interpreters in this build may not support the selected stable ABI
                        // family, e.g. 3.14t when abi3t was selected for 3.15.
                        let version_specific_tags = version_specific_interps
                            .iter()
                            .map(|interpreter| {
                                interpreter
                                    .get_wheel_tag(&self.context.project, &[PlatformTag::Linux])
                            })
                            .collect::<Result<Vec<_>>>()?;
                        let tags = stable_abi_tag
                            .into_iter()
                            .chain(version_specific_tags)
                            .collect::<Vec<_>>();
                        if tags.is_empty() {
                            bail!("No compatible Python interpreters found for stable ABI build");
                        }
                        tags
                    }
                    None => {
                        vec![interp.get_wheel_tag(&self.context.project, &[PlatformTag::Linux])?]
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

    /// Returns the universal Python 3 wheel tag for the given platform tags.
    fn get_universal_tag(&self, platform_tags: &[PlatformTag]) -> Result<WheelTag> {
        let platform = self.context.project.get_platform_tag(platform_tags)?;
        Ok(WheelTag::new("py3", "none", platform))
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

    /// Build at most one stable ABI wheel, with non-matching interpreters
    /// falling back to version-specific wheels.
    #[instrument(skip_all)]
    pub(crate) fn build_stable_abi_wheels(
        &self,
        stable_abi: &StableAbi,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheel>> {
        let min_version = stable_abi.version.min_version();
        // With mixed abi3 and abi3t features, selecting abi3t for a 3.15+
        // interpreter intentionally sends older interpreters to version-specific wheels.
        let bridge = self.context.project.bridge();
        let (stable_abi_interps, version_specific_abi_interps): (Vec<_>, Vec<_>) = self
            .context
            .python
            .interpreter
            .iter()
            .partition(|interp| bridge.is_stable_abi_for_interpreter(interp));

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
            if let Some((major, minor)) = stable_abi.version.min_version() {
                bail!(
                    "None of the found Python interpreters ({}) are compatible with the {} \
                     minimum version (>= {}.{}). Please install a compatible Python interpreter.",
                    interp_names.join(", "),
                    stable_abi.kind,
                    major,
                    minor,
                );
            } else {
                bail!(
                    "No compatible Python interpreters found for {} build. \
                     Found: {}",
                    stable_abi.kind,
                    interp_names.join(", "),
                );
            }
        }

        let mut built_wheels = Vec::new();
        if let Some(first) = stable_abi_interps.first() {
            let (major, minor) = min_version.unwrap_or((first.major as u8, first.minor as u8));
            built_wheels.extend(self.build_single_unit(Some(first), |orchestrator| {
                orchestrator.build_pyo3_wheel_stable_abi(
                    &stable_abi_interps,
                    *stable_abi,
                    major,
                    minor,
                    sbom_data,
                )
            })?);
        }
        if !version_specific_abi_interps.is_empty() {
            let interp_names: HashSet<_> = version_specific_abi_interps
                .iter()
                .map(|interp| interp.to_string())
                .collect();
            ui::warning!(
                "⚠️ Warning: {} does not yet support {} so the build artifacts will be version-specific.",
                stable_abi.kind,
                interp_names.iter().join(", ")
            );
            built_wheels.extend(self.build_pyo3_wheels(&version_specific_abi_interps, sbom_data)?);
        }
        Ok(built_wheels)
    }

    /// Single shared wheel-writing pipeline used by extension and binary wheels.
    ///
    /// `metadata24` is a parameter because binary wheels write a locally mutated metadata copy.
    #[allow(clippy::too_many_arguments)]
    fn write_wheel_inner<F, G>(
        &self,
        tag: &WheelTag,
        metadata24: &Metadata24,
        audited: &mut [AuditedArtifact],
        use_external_lib_shim: bool,
        make_generator: F,
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<PathBuf>
    where
        F: FnOnce(Rc<tempfile::TempDir>) -> Result<G>,
        G: BindingGenerator,
    {
        let file_options = self
            .context
            .artifact
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let tag_string = tag.to_string();
        let writer = WheelWriter::new(
            &tag_string,
            &self.context.artifact.out,
            metadata24,
            file_options,
        )?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);
        self.context
            .add_external_libs(&mut writer, audited, use_external_lib_shim)?;

        let temp_dir = writer.temp_dir()?;
        let mut generator = make_generator(temp_dir)?;
        generate_binding(&mut writer, &mut generator, self.context, audited, out_dirs)
            .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            metadata24,
            self.context.project.project_layout.data.as_deref(),
        )?;

        SbomData::write(
            sbom_data.as_ref(),
            self.context,
            &mut writer,
            &metadata24.get_dist_info_dir(),
        )?;

        let wheel_path = writer.finish(
            metadata24,
            &self.context.project.project_layout.project_root,
            std::slice::from_ref(tag),
        )?;
        finalize_staged_artifacts(audited);
        Ok(wheel_path)
    }

    /// The extension-wheel writing loop. Handles metadata generation, file compression,
    /// and writing the final .whl archive to the output directory.
    fn write_wheel<F, G>(
        &self,
        tag: &WheelTag,
        audited: &mut [AuditedArtifact],
        make_generator: F,
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<PathBuf>
    where
        F: FnOnce(Rc<tempfile::TempDir>) -> Result<G>,
        G: BindingGenerator,
    {
        self.write_wheel_inner(
            tag,
            &self.context.project.metadata24,
            audited,
            false,
            make_generator,
            sbom_data,
            out_dirs,
        )
    }

    /// For abi3 and abi3t we only need to build a single wheel and we don't
    /// even need a python interpreter for it
    #[instrument(skip_all)]
    pub(crate) fn build_pyo3_wheel_stable_abi(
        &self,
        interpreters: &[&PythonInterpreter],
        stable_abi: StableAbi,
        major: u8,
        min_minor: u8,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheel>> {
        let mut wheels = Vec::new();
        let python_interpreter = interpreters.first().copied();
        let (audited_artifact, policy, out_dirs) = self.compile_and_audit(
            python_interpreter,
            Some(&self.context.project.project_layout.extension_name),
        )?;
        let platform_tags = self.resolve_platform_tags(&policy);

        let platform = self.context.project.get_platform_tag(&platform_tags)?;
        let abi_tag = stable_abi.kind.wheel_tag();
        let tag = WheelTag::new(format!("cp{major}{min_minor}"), abi_tag, platform);

        let mut audited = [audited_artifact];
        let wheel_path = self.write_wheel(
            &tag,
            &mut audited,
            |temp_dir| {
                Ok(Pyo3BindingGenerator::new_stable_abi(
                    stable_abi.kind,
                    python_interpreter,
                    temp_dir,
                ))
            },
            sbom_data,
            &out_dirs,
        )?;

        ui::status!(
            "📦 Built wheel for {} Python ≥ {}.{} to {}",
            stable_abi.kind,
            major,
            min_minor,
            wheel_path.display()
        );
        wheels.push(BuiltWheel {
            path: wheel_path,
            tag: BuiltArtifactTag::interpreter(tag.python()),
        });

        Ok(wheels)
    }

    /// Writes a PyO3 wheel for a specific Python interpreter.
    fn write_pyo3_wheel(
        &self,
        python_interpreter: &PythonInterpreter,
        audited: &mut [AuditedArtifact],
        platform_tags: &[PlatformTag],
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<(PathBuf, WheelTag)> {
        let tag = python_interpreter.get_wheel_tag(&self.context.project, platform_tags)?;

        let path = self.write_wheel(
            &tag,
            audited,
            |temp_dir| {
                Ok(Pyo3BindingGenerator::new_version_specific(
                    python_interpreter,
                    temp_dir,
                ))
            },
            sbom_data,
            out_dirs,
        )?;
        Ok((path, tag))
    }

    /// Compile, audit, and write a single PyO3 wheel for one interpreter.
    #[instrument(skip_all)]
    pub(crate) fn build_single_pyo3_wheel(
        &self,
        python_interpreter: &PythonInterpreter,
        sbom_data: &Option<SbomData>,
    ) -> Result<BuiltWheel> {
        let (audited_artifact, policy, out_dirs) = self.compile_and_audit(
            Some(python_interpreter),
            Some(&self.context.project.project_layout.extension_name),
        )?;
        let platform_tags = self.resolve_platform_tags(&policy);
        let mut audited = [audited_artifact];
        let (wheel_path, tag) = self.write_pyo3_wheel(
            python_interpreter,
            &mut audited,
            &platform_tags,
            sbom_data,
            &out_dirs,
        )?;
        Ok(BuiltWheel {
            path: wheel_path,
            tag: BuiltArtifactTag::interpreter(tag.python()),
        })
    }

    /// Builds wheels for a pyo3 extension for all given python versions.
    #[instrument(skip_all)]
    pub(crate) fn build_pyo3_wheels(
        &self,
        interpreters: &[&PythonInterpreter],
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheel>> {
        let mut wheels = Vec::new();
        for &python_interpreter in interpreters {
            let mut unit_wheels =
                self.build_single_unit(Some(python_interpreter), |orchestrator| {
                    Ok(vec![
                        orchestrator.build_single_pyo3_wheel(python_interpreter, sbom_data)?,
                    ])
                })?;
            for wheel in &unit_wheels {
                ui::status!(
                    "📦 Built wheel for {} {}.{}{} to {}",
                    python_interpreter.interpreter_kind,
                    python_interpreter.major,
                    python_interpreter.minor,
                    python_interpreter.abiflags,
                    wheel.path.display()
                );
            }
            wheels.append(&mut unit_wheels);
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
        let artifacts = result
            .artifacts
            .first()
            .with_context(|| missing_cdylib_message(None))?;

        let mut artifact = artifacts
            .get(&CrateType::CDyLib)
            .cloned()
            .ok_or_else(|| missing_cdylib_error(None))?;

        self.context.stage_artifact(&mut artifact)?;

        if let Some(extension_name) = extension_name {
            let _ = warn_missing_py_init(&artifact.path, extension_name);
        }
        Ok((artifact, result.out_dirs))
    }

    /// Compiles the cdylib, runs auditwheel, and bundles the audited artifact,
    /// selected policy, and OUT_DIR map for wheel writing.
    fn compile_and_audit(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        extension_name: Option<&str>,
    ) -> Result<(AuditedArtifact, Policy, HashMap<String, PathBuf>)> {
        let (artifact, out_dirs) = self.compile_cdylib(python_interpreter, extension_name)?;
        let audit_result = self.context.auditwheel(
            &artifact,
            &self.context.python.platform_tag,
            python_interpreter,
        )?;
        Ok((
            AuditedArtifact {
                artifact,
                external_libs: audit_result.external_libs,
                arch_requirements: audit_result.arch_requirements,
            },
            audit_result.policy,
            out_dirs,
        ))
    }

    /// Compiles a cdylib and builds a wheel for it.
    fn build_cdylib_wheel<F, G>(
        &self,
        make_generator: F,
        sbom_data: &Option<SbomData>,
    ) -> Result<(PathBuf, HashMap<String, PathBuf>)>
    where
        F: FnOnce(Rc<tempfile::TempDir>) -> Result<G>,
        G: BindingGenerator,
    {
        let (audited_artifact, policy, out_dirs) = self.compile_and_audit(None, None)?;
        let platform_tags = self.resolve_platform_tags(&policy);
        let tag = self.get_universal_tag(&platform_tags)?;
        let mut audited = [audited_artifact];
        let wheel_path =
            self.write_wheel(&tag, &mut audited, make_generator, sbom_data, &out_dirs)?;
        Ok((wheel_path, out_dirs))
    }

    /// Builds a wheel with cffi bindings
    #[instrument(skip_all)]
    fn build_cffi_wheel(&self, sbom_data: &Option<SbomData>) -> Result<Vec<BuiltWheel>> {
        let interpreter = self.context.python.interpreter.first().ok_or_else(|| {
            anyhow!("A python interpreter is required for cffi builds but one was not provided")
        })?;
        let (wheel_path, _) = self.build_cdylib_wheel(
            |temp_dir| {
                CffiBindingGenerator::new(interpreter, temp_dir)
                    .context("Failed to initialize Cffi binding generator")
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
            ui::warning!(
                "⚠️  Warning: missing cffi package dependency, please add it to pyproject.toml. \
                e.g: `dependencies = [\"cffi\"]`. This will become an error."
            );
        }

        ui::status!("📦 Built wheel to {}", wheel_path.display());
        Ok(vec![BuiltWheel {
            path: wheel_path,
            tag: BuiltArtifactTag::Universal,
        }])
    }

    /// Builds a wheel with uniffi bindings
    #[instrument(skip_all)]
    fn build_uniffi_wheel(&self, sbom_data: &Option<SbomData>) -> Result<Vec<BuiltWheel>> {
        let (wheel_path, _) =
            self.build_cdylib_wheel(|_temp_dir| Ok(UniFfiBindingGenerator::default()), sbom_data)?;

        ui::status!("📦 Built wheel to {}", wheel_path.display());
        Ok(vec![BuiltWheel {
            path: wheel_path,
            tag: BuiltArtifactTag::Universal,
        }])
    }

    /// Internal implementation for writing a binary wheel.
    fn write_bin_wheel(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        audited: &mut [AuditedArtifact],
        platform_tags: &[PlatformTag],
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<(PathBuf, WheelTag)> {
        if !self.context.project.metadata24.scripts.is_empty() {
            bail!("Defining scripts and working with a binary doesn't mix well");
        }

        if self.context.project.target.is_wasi() {
            ui::warning!("⚠️  Warning: wasi support is experimental");
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
                python_interpreter.get_wheel_tag(&self.context.project, platform_tags)?
            }
            _ => unreachable!(),
        };

        let mut metadata24 = self.context.project.metadata24.clone();
        // When repair mode bundles external shared library dependencies, use
        // the shim approach: move the real binary to {dist}.scripts/ in
        // platlib (where it has a predictable relative path to the bundled
        // libs directory) and place a Python shim in .data/scripts/ that execs
        // the real binary.
        // WASI targets use their own launcher mechanism and cannot be shimmed.
        let has_external_libs = audited.iter().any(|a| !a.external_libs.is_empty());
        let use_shim = should_use_bin_shim(
            self.context.python.auditwheel,
            self.context.project.target.is_wasi(),
            has_external_libs,
        );
        BinBindingGenerator::prepare_metadata(&mut metadata24, self.context, audited)
            .context("Failed to add the files to the wheel")?;

        let path = self.write_wheel_inner(
            &tag,
            &metadata24,
            audited,
            use_shim,
            |_temp_dir| Ok(BinBindingGenerator::new(&metadata24, use_shim)),
            sbom_data,
            out_dirs,
        )?;
        Ok((path, tag))
    }

    /// Builds a wheel that contains a binary
    #[instrument(skip_all)]
    fn build_bin_wheel(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheel>> {
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

        let (wheel_path, tag) = self.write_bin_wheel(
            python_interpreter,
            &mut audited_artifacts,
            &platform_tags,
            sbom_data,
            &result.out_dirs,
        )?;
        ui::status!("📦 Built wheel to {}", wheel_path.display());
        // Interpreter-bound binary wheels (pyo3-bin) carry a real python tag; pure
        // standalone bins remain universal (`py3`).
        let artifact_tag = if python_interpreter.is_some() {
            BuiltArtifactTag::interpreter(tag.python())
        } else {
            BuiltArtifactTag::Universal
        };
        wheels.push(BuiltWheel {
            path: wheel_path,
            tag: artifact_tag,
        });

        Ok(wheels)
    }

    /// Builds wheels for a binary project for all given python versions.
    #[instrument(skip_all)]
    pub(crate) fn build_bin_wheels(
        &self,
        interpreters: &[&PythonInterpreter],
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheel>> {
        let mut wheels = Vec::new();
        for &python_interpreter in interpreters {
            wheels.extend(
                self.build_single_unit(Some(python_interpreter), |orchestrator| {
                    orchestrator.build_bin_wheel(Some(python_interpreter), sbom_data)
                })?,
            );
        }
        Ok(wheels)
    }

    /// Generate stub files by building the project then extracting the stubs from the build output
    #[instrument(skip_all)]
    pub fn generate_stubs(&self) -> Result<HashMap<PathBuf, String>> {
        match self.context.project.bridge() {
            BridgeModel::PyO3(_) => {
                let python_interpreter = self.context.python.interpreter.first();
                let extension_name = &self.context.project.project_layout.extension_name;
                let (artifact, _) =
                    self.compile_cdylib(python_interpreter, Some(extension_name))?;
                let module_introspection = introspect_cdylib(&artifact.path, extension_name).context("Failed to introspect the built libraries to generate type stubs, have you enabled the \"experimental-inspect\" PyO3 Cargo feature?")?;
                Ok(module_stub_files(&module_introspection))
            }
            _ => bail!("Stub generation is only possible in PyO3 projects"),
        }
    }
}

fn should_use_bin_shim(auditwheel: AuditWheelMode, is_wasi: bool, has_external_libs: bool) -> bool {
    matches!(auditwheel, AuditWheelMode::Repair) && !is_wasi && has_external_libs
}

#[cfg(test)]
mod tests {
    use super::should_use_bin_shim;
    use crate::auditwheel::AuditWheelMode;

    #[test]
    fn test_should_use_bin_shim_only_for_repair_with_external_libs() {
        assert!(should_use_bin_shim(AuditWheelMode::Repair, false, true));

        assert!(!should_use_bin_shim(AuditWheelMode::Warn, false, true));
        assert!(!should_use_bin_shim(AuditWheelMode::Check, false, true));
        assert!(!should_use_bin_shim(AuditWheelMode::Skip, false, true));
        assert!(!should_use_bin_shim(AuditWheelMode::Repair, true, true));
        assert!(!should_use_bin_shim(AuditWheelMode::Repair, false, false));
    }
}
