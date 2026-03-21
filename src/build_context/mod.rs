mod builder;
mod repair;
mod wheels;

pub use builder::BuildContextBuilder;

use crate::auditwheel::AuditWheelMode;
use crate::auditwheel::{PlatformTag, Policy};
use crate::bridge::Abi3Version;
use crate::build_options::CargoOptions;
use crate::compile::CompileTarget;
use crate::compression::CompressionOptions;
use crate::module_writer::{WheelWriter, write_pth};
use crate::pgo::{PgoContext, PgoPhase};
use crate::project_layout::ProjectLayout;
use crate::pyproject_toml::ConditionalFeature;
use crate::sbom::generate_sbom_data;
use crate::source_distribution::source_distribution;
use crate::target::validate_wheel_filename_for_pypi;
use crate::{
    BridgeModel, Metadata24, PyProjectToml, PythonInterpreter, Target, VirtualWriter,
    pyproject_toml::Format, pyproject_toml::SbomConfig,
};
use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::Metadata;
use fs_err as fs;
use ignore::overrides::{Override, OverrideBuilder};
use normpath::PathExt;
use std::path::PathBuf;
use tracing::instrument;

/// Project context
#[derive(Clone, Debug)]
pub struct ProjectContext {
    /// Whether this project is pure rust or rust mixed with python
    pub project_layout: ProjectLayout,
    /// The path to pyproject.toml. Required for the source distribution
    pub pyproject_toml_path: PathBuf,
    /// Parsed pyproject.toml if any
    pub pyproject_toml: Option<PyProjectToml>,
    /// Python Package Metadata 2.4
    pub metadata24: Metadata24,
    /// The name of the crate
    pub crate_name: String,
    /// The name of the module
    pub module_name: String,
    /// The path to the Cargo.toml. Required for the cargo invocations
    pub manifest_path: PathBuf,
    /// Directory for all generated artifacts
    pub target_dir: PathBuf,
    /// Cargo.toml as resolved by [cargo_metadata]
    pub cargo_metadata: Metadata,
}

/// Artifact context
#[derive(Clone, Debug)]
pub struct ArtifactContext {
    /// The directory to store the built wheels in
    pub out: PathBuf,
    /// Strip the library for minimum file size
    pub strip: bool,
    /// Compression options
    pub compression: CompressionOptions,
    /// SBOM configuration
    pub sbom: Option<SbomConfig>,
    /// Include the import library in the wheel on Windows
    pub include_import_lib: bool,
    /// Include debug info files (.pdb, .dSYM, .dwp) in the wheel
    pub include_debuginfo: bool,
}

/// Python context
#[derive(Clone, Debug)]
pub struct PythonContext {
    /// Checking the linked libraries for manylinux compliance
    pub auditwheel: AuditWheelMode,
    /// When compiling for manylinux, use zig as linker
    #[cfg(feature = "zig")]
    pub zig: bool,
    /// Whether to use the manylinux/musllinux or use the native linux tag
    pub platform_tag: Vec<PlatformTag>,
    /// The available python interpreters
    pub interpreter: Vec<PythonInterpreter>,
    /// Whether to validate wheels against PyPI platform tag rules
    pub pypi_validation: bool,
}

/// Contains all the metadata required to build the crate
#[derive(Clone)]
pub struct BuildContext {
    /// The platform, i.e. os and pointer width
    pub target: Target,
    /// List of Cargo targets to compile
    pub compile_targets: Vec<CompileTarget>,
    /// Project context
    pub project: ProjectContext,
    /// Artifact context
    pub artifact: ArtifactContext,
    /// Python context
    pub python: PythonContext,
    /// Whether to use universal2 or use the native macOS tag (off)
    pub universal2: bool,
    /// Build editable wheels
    pub editable: bool,
    /// Cargo build options
    pub cargo_options: CargoOptions,
    /// Cargo features conditionally enabled based on the target Python version/implementation
    pub conditional_features: Vec<ConditionalFeature>,
    /// Current PGO build phase (if PGO is enabled)
    pub pgo_phase: Option<PgoPhase>,
    /// PGO training command from pyproject.toml (only set when --pgo is passed)
    pub pgo_command: Option<String>,
}

/// The wheel file location and its Python version tag (e.g. `py3`).
///
/// For bindings the version tag contains the Python interpreter version
/// they bind against (e.g. `cp37`).
pub type BuiltWheelMetadata = (PathBuf, String);

impl BuildContext {
    /// Checks which kind of bindings we have (pyo3/rust-cypthon or cffi or bin) and calls the
    /// correct builder.
    #[instrument(skip_all)]
    pub fn build_wheels(&self) -> Result<Vec<BuiltWheelMetadata>> {
        if self.pgo_command.is_some() {
            return self.build_wheels_pgo();
        }
        self.build_wheels_inner()
    }

    /// Standard wheel build pipeline (no PGO).
    fn build_wheels_inner(&self) -> Result<Vec<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.artifact.out)
            .context("Failed to create the target directory for the wheels")?;

        // Generate SBOM data once for all wheels (the Rust dependency graph
        // is the same regardless of the target Python interpreter).
        let sbom_data = generate_sbom_data(self)?;

        let wheels = match self.bridge() {
            BridgeModel::Bin(None) => self.build_bin_wheel(None, &sbom_data)?,
            BridgeModel::Bin(Some(..)) => {
                self.build_bin_wheels(&self.python.interpreter, &sbom_data)?
            }
            BridgeModel::PyO3(crate::PyO3 { abi3, .. }) => match abi3 {
                Some(Abi3Version::Version(major, minor)) => {
                    self.build_abi3_wheels(Some((*major, *minor)), &sbom_data)?
                }
                Some(Abi3Version::CurrentPython) => self.build_abi3_wheels(None, &sbom_data)?,
                None => self.build_pyo3_wheels(&self.python.interpreter, &sbom_data)?,
            },
            BridgeModel::Cffi => self.build_cffi_wheel(&sbom_data)?,
            BridgeModel::UniFfi => self.build_uniffi_wheel(&sbom_data)?,
        };

        // Validate wheel filenames against PyPI platform tag rules if requested
        if self.python.pypi_validation {
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

    /// PGO three-phase build: instrumented → instrumentation → optimized.
    ///
    /// For non-abi3 PyO3 builds each interpreter gets its own
    /// instrument → train → optimize cycle, because different Python
    /// versions produce different compiled code via PyO3's build-time
    /// configuration, so sharing a single profile across interpreters
    /// causes "function control flow change detected (hash mismatch)"
    /// warnings and defeats the purpose of PGO.
    ///
    /// For single-artifact builds (abi3, cffi, uniffi, bin) the compiled
    /// code is identical across interpreters, so one PGO cycle suffices.
    fn build_wheels_pgo(&self) -> Result<Vec<BuiltWheelMetadata>> {
        let pgo_command = self
            .pgo_command
            .as_ref()
            .expect("pgo_command must be set when build_wheels_pgo is called");

        let needs_per_interpreter_pgo = matches!(
            self.bridge(),
            BridgeModel::PyO3(crate::PyO3 { abi3: None, .. })
        );

        eprintln!("🚀 Starting PGO build...");

        // Verify llvm-profdata is available before starting
        PgoContext::find_llvm_profdata()?;

        if needs_per_interpreter_pgo {
            self.build_wheels_pgo_per_interpreter(pgo_command)
        } else {
            self.build_wheels_pgo_single_pass(pgo_command)
        }
    }

    /// Clone this context with PGO disabled (to prevent recursion) and
    /// the given PGO phase set.
    fn clone_for_pgo(&self, phase: PgoPhase) -> Self {
        let mut ctx = self.clone();
        ctx.pgo_command = None;
        ctx.pgo_phase = Some(phase);
        ctx
    }

    /// Single-pass PGO for abi3, cffi, uniffi, and bin builds where the
    /// compiled artifact is the same regardless of the Python interpreter.
    fn build_wheels_pgo_single_pass(&self, pgo_command: &str) -> Result<Vec<BuiltWheelMetadata>> {
        let instrumentation_python = self
            .python
            .interpreter
            .first()
            .context(
                "PGO builds require a Python interpreter. \
                 Please specify one with `--interpreter`.",
            )?
            .executable
            .clone();

        let pgo_ctx = PgoContext::new(pgo_command.to_owned())?;

        // Phase 1: Build a single instrumented wheel for training.
        // We only need one wheel for profiling — the compiled native code is
        // identical across interpreters for abi3/cffi/uniffi/bin builds.
        eprintln!("📊 Phase 1/3: Building instrumented wheel...");
        let mut instrumented_ctx = self.clone_for_pgo(PgoPhase::Generate(
            pgo_ctx.profdata_dir_path().to_path_buf(),
        ));
        instrumented_ctx.python.interpreter = vec![self.python.interpreter[0].clone()];
        let instrumented_out =
            tempfile::TempDir::new().context("Failed to create temp dir for instrumented wheel")?;
        instrumented_ctx.artifact.out = instrumented_out.path().to_path_buf();
        let instrumented_wheels = instrumented_ctx.build_wheels_inner()?;

        // Phase 2: Instrumentation
        eprintln!("🔬 Phase 2/3: Running PGO instrumentation...");
        let instrumented_wheel_path = &instrumented_wheels
            .first()
            .context("No instrumented wheel was built")?
            .0;
        pgo_ctx.run_instrumentation(&instrumentation_python, instrumented_wheel_path, self)?;
        pgo_ctx.merge_profiles()?;

        // Phase 3: Optimized build
        eprintln!("⚡ Phase 3/3: Building PGO-optimized wheel...");
        let optimized_ctx =
            self.clone_for_pgo(PgoPhase::Use(pgo_ctx.merged_profdata_path().to_path_buf()));
        let wheels = optimized_ctx.build_wheels_inner()?;

        eprintln!("🎉 PGO build complete!");
        Ok(wheels)
    }

    /// Per-interpreter PGO for non-abi3 PyO3 builds. Each interpreter gets
    /// its own instrument → train → optimize cycle so that profiles match
    /// the exact compiled code for that Python version.
    fn build_wheels_pgo_per_interpreter(
        &self,
        pgo_command: &str,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.artifact.out)
            .context("Failed to create the target directory for the wheels")?;

        let sbom_data = generate_sbom_data(self)?;
        let mut wheels = Vec::new();

        for (i, python_interpreter) in self.python.interpreter.iter().enumerate() {
            eprintln!(
                "📊 [{}/{}] PGO cycle for {} {}.{}...",
                i + 1,
                self.python.interpreter.len(),
                python_interpreter.interpreter_kind,
                python_interpreter.major,
                python_interpreter.minor,
            );

            let pgo_ctx = PgoContext::new(pgo_command.to_owned())?;

            // Phase 1: Build instrumented wheel for this interpreter
            eprintln!("  📊 Phase 1/3: Building instrumented wheel...");
            let mut instrumented_ctx = self.clone_for_pgo(PgoPhase::Generate(
                pgo_ctx.profdata_dir_path().to_path_buf(),
            ));
            let instrumented_out = tempfile::TempDir::new()
                .context("Failed to create temp dir for instrumented wheel")?;
            instrumented_ctx.artifact.out = instrumented_out.path().to_path_buf();
            let (instrumented_wheel_path, _) =
                instrumented_ctx.build_single_pyo3_wheel(python_interpreter, &sbom_data)?;

            // Phase 2: Run instrumentation with this interpreter
            eprintln!("  🔬 Phase 2/3: Running PGO instrumentation...");
            pgo_ctx.run_instrumentation(
                &python_interpreter.executable,
                &instrumented_wheel_path,
                self,
            )?;
            pgo_ctx.merge_profiles()?;

            // Phase 3: Build optimized wheel for this interpreter
            eprintln!("  ⚡ Phase 3/3: Building PGO-optimized wheel...");
            let optimized_ctx =
                self.clone_for_pgo(PgoPhase::Use(pgo_ctx.merged_profdata_path().to_path_buf()));
            let (wheel_path, tag) =
                optimized_ctx.build_single_pyo3_wheel(python_interpreter, &sbom_data)?;

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
        if self.python.pypi_validation {
            for (wheel_path, _) in &wheels {
                let filename = wheel_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| anyhow!("Invalid wheel filename: {:?}", wheel_path))?;

                if let Err(error) = validate_wheel_filename_for_pypi(filename) {
                    bail!("PyPI validation failed: {}", error);
                }
            }
        }

        eprintln!("🎉 PGO build complete!");
        Ok(wheels)
    }

    /// Bridge model
    pub fn bridge(&self) -> &BridgeModel {
        // FIXME: currently we only allow multiple bin targets so bridges are all the same
        &self.compile_targets[0].bridge_model
    }

    /// Builds a source distribution and returns the same metadata as [BuildContext::build_wheels]
    pub fn build_source_distribution(&self) -> Result<Option<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.artifact.out)
            .context("Failed to create the target directory for the source distribution")?;

        match self.project.pyproject_toml.as_ref() {
            Some(pyproject) => {
                let sdist_path =
                    source_distribution(self, pyproject, self.excludes(Format::Sdist)?)
                        .context("Failed to build source distribution")?;
                Ok(Some((sdist_path, "source".to_string())))
            }
            None => Ok(None),
        }
    }

    /// Return the tags of the wheel that this build context builds.
    pub fn tags_from_bridge(&self) -> Result<Vec<String>> {
        let tags = match self.bridge() {
            BridgeModel::PyO3(bindings) | BridgeModel::Bin(Some(bindings)) => match bindings.abi3 {
                Some(Abi3Version::Version(major, minor)) => {
                    let platform = self.get_platform_tag(&[PlatformTag::Linux])?;
                    vec![format!("cp{major}{minor}-abi3-{platform}")]
                }
                Some(Abi3Version::CurrentPython) => {
                    let interp = &self.python.interpreter[0];
                    let platform = self.get_platform_tag(&[PlatformTag::Linux])?;
                    vec![format!(
                        "cp{major}{minor}-abi3-{platform}",
                        major = interp.major,
                        minor = interp.minor
                    )]
                }
                None => {
                    vec![self.python.interpreter[0].get_tag(self, &[PlatformTag::Linux])?]
                }
            },
            BridgeModel::Bin(None) | BridgeModel::Cffi | BridgeModel::UniFfi => {
                vec![self.get_universal_tag(&[PlatformTag::Linux])?]
            }
        };
        Ok(tags)
    }

    fn add_pth(&self, writer: &mut VirtualWriter<WheelWriter>) -> Result<()> {
        if self.editable {
            write_pth(
                writer,
                &self.project.project_layout,
                &self.project.metadata24,
            )?;
        }
        Ok(())
    }

    fn excludes(&self, format: Format) -> Result<Override> {
        let project_dir = match self.project.pyproject_toml_path.normalize() {
            Ok(pyproject_toml_path) => pyproject_toml_path.into_path_buf(),
            Err(_) => self.project.manifest_path.normalize()?.into_path_buf(),
        };
        let mut excludes = OverrideBuilder::new(project_dir.parent().unwrap());
        if let Some(pyproject) = self.project.pyproject_toml.as_ref()
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
                self.artifact.out.display(),
                std::path::MAIN_SEPARATOR,
                &self.project.metadata24.get_distribution_escaped(),
            );
            excludes.add(&glob_pattern)?;
        }
        Ok(excludes.build()?)
    }

    /// Returns the platform part of the tag for the wheel name
    pub fn get_platform_tag(&self, platform_tags: &[PlatformTag]) -> Result<String> {
        crate::target::get_platform_tag(
            &self.target,
            platform_tags,
            self.universal2,
            self.project.pyproject_toml.as_ref(),
            &self.project.manifest_path,
        )
    }

    /// Returns the platform tag without python version (e.g. `py3-none-manylinux_2_17_x86_64`)
    pub fn get_universal_tag(&self, platform_tags: &[PlatformTag]) -> Result<String> {
        let platform = self.get_platform_tag(platform_tags)?;
        Ok(format!("py3-none-{platform}"))
    }

    /// Returns user-specified platform tags, or falls back to the auditwheel
    /// policy tag when no explicit tags were provided.
    fn resolve_platform_tags(&self, policy: &Policy) -> Vec<PlatformTag> {
        if self.python.platform_tag.is_empty() {
            vec![policy.platform_tag()]
        } else {
            self.python.platform_tag.clone()
        }
    }
}
