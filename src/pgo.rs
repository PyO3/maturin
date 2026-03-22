use crate::develop::install_backend::{find_uv_bin, find_uv_python};
use crate::{BridgeModel, BuildOrchestrator, BuiltWheelMetadata};
use anyhow::{Context, Result, anyhow, bail};
use fs_err as fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use tracing::debug;

/// The current phase of a PGO build
#[derive(Debug, Clone)]
pub enum PgoPhase {
    /// Instrumented build: `-Cprofile-generate=<dir>`
    Generate(PathBuf),
    /// Optimized build: `-Cprofile-use=<path>`
    Use(PathBuf),
}

/// Manages the PGO build lifecycle: profdata directory, llvm-profdata resolution, instrumentation
pub struct PgoContext {
    /// Temporary directory for .profraw files
    profdata_dir: TempDir,
    /// Path to the merged .profdata file
    merged_profdata: PathBuf,
    /// The instrumentation command to run
    pgo_command: String,
}

impl PgoContext {
    /// Create a new PGO context with a temporary directory for profile data
    pub fn new(pgo_command: String) -> Result<Self> {
        let profdata_dir =
            TempDir::new().context("Failed to create temporary directory for PGO profdata")?;
        let merged_profdata = profdata_dir.path().join("merged.profdata");
        Ok(Self {
            profdata_dir,
            merged_profdata,
            pgo_command,
        })
    }

    /// Returns the path to the profdata directory
    pub fn profdata_dir_path(&self) -> &Path {
        self.profdata_dir.path()
    }

    /// Returns the path to the merged profdata file
    pub fn merged_profdata_path(&self) -> &Path {
        &self.merged_profdata
    }

    /// Orchestrate a three-phase PGO build: instrumented → instrumentation → optimized.
    pub fn build_wheels_pgo(
        orchestrator: &BuildOrchestrator,
        pgo_command: String,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let needs_per_interpreter_pgo = matches!(
            orchestrator.context().project.bridge(),
            BridgeModel::PyO3(crate::PyO3 { abi3: None, .. })
        );

        eprintln!("🚀 Starting PGO build...");

        // Verify llvm-profdata is available before starting
        Self::find_llvm_profdata()?;

        if needs_per_interpreter_pgo {
            Self::build_wheels_pgo_per_interpreter(orchestrator, pgo_command)
        } else {
            let pgo_ctx = Self::new(pgo_command)?;
            pgo_ctx.build_wheels_pgo_single_pass(orchestrator)
        }
    }

    /// Single-pass PGO for abi3, cffi, uniffi, and bin builds.
    fn build_wheels_pgo_single_pass(
        &self,
        orchestrator: &BuildOrchestrator,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let context = orchestrator.context();
        let instrumentation_python = context
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
        let mut instrumented_ctx = orchestrator
            .clone_context_for_pgo(PgoPhase::Generate(self.profdata_dir_path().to_path_buf()));
        instrumented_ctx.python.interpreter = vec![context.python.interpreter[0].clone()];
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
        self.run_instrumentation(&instrumentation_python, instrumented_wheel_path, context)?;
        self.merge_profiles()?;

        // Phase 3: Optimized build
        eprintln!("⚡ Phase 3/3: Building PGO-optimized wheel...");
        let optimized_ctx = orchestrator
            .clone_context_for_pgo(PgoPhase::Use(self.merged_profdata_path().to_path_buf()));
        let optimized_orchestrator = BuildOrchestrator::new(&optimized_ctx);
        let wheels = optimized_orchestrator.build_wheels_inner()?;

        eprintln!("🎉 PGO build complete!");
        Ok(wheels)
    }

    /// Per-interpreter PGO for non-abi3 PyO3 builds.
    fn build_wheels_pgo_per_interpreter(
        orchestrator: &BuildOrchestrator,
        pgo_command: String,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let context = orchestrator.context();
        fs::create_dir_all(&context.artifact.out)
            .context("Failed to create the target directory for the wheels")?;

        let sbom_data = orchestrator.generate_sbom_data()?;
        let mut wheels = Vec::new();

        for (i, python_interpreter) in context.python.interpreter.iter().enumerate() {
            eprintln!(
                "📊 [{}/{}] PGO cycle for {} {}.{}...",
                i + 1,
                context.python.interpreter.len(),
                python_interpreter.interpreter_kind,
                python_interpreter.major,
                python_interpreter.minor,
            );

            // Create a fresh PGO context for each interpreter cycle
            let pgo_cycle_ctx = Self::new(pgo_command.clone())?;

            // Phase 1: Build instrumented wheel for this interpreter
            eprintln!("  📊 Phase 1/3: Building instrumented wheel...");
            let mut instrumented_ctx = orchestrator.clone_context_for_pgo(PgoPhase::Generate(
                pgo_cycle_ctx.profdata_dir_path().to_path_buf(),
            ));
            let instrumented_out = tempfile::TempDir::new()
                .context("Failed to create temp dir for instrumented wheel")?;
            instrumented_ctx.artifact.out = instrumented_out.path().to_path_buf();

            let instrumented_orchestrator = BuildOrchestrator::new(&instrumented_ctx);
            let (instrumented_wheel_path, _) = instrumented_orchestrator
                .build_single_pyo3_wheel(python_interpreter, &sbom_data)?;

            // Phase 2: Run instrumentation with this interpreter
            eprintln!("  🔬 Phase 2/3: Running PGO instrumentation...");
            pgo_cycle_ctx.run_instrumentation(
                &python_interpreter.executable,
                &instrumented_wheel_path,
                context,
            )?;
            pgo_cycle_ctx.merge_profiles()?;

            // Phase 3: Build optimized wheel for this interpreter
            eprintln!("  ⚡ Phase 3/3: Building PGO-optimized wheel...");
            let optimized_ctx = orchestrator.clone_context_for_pgo(PgoPhase::Use(
                pgo_cycle_ctx.merged_profdata_path().to_path_buf(),
            ));
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
        if context.python.pypi_validation {
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

    /// Find the `llvm-profdata` binary.
    ///
    /// Strategy:
    /// 1. Look in the rustup toolchain sysroot
    /// 2. Fall back to PATH
    pub fn find_llvm_profdata() -> Result<PathBuf> {
        // Try rustup toolchain first
        if let Ok(path) = Self::find_llvm_profdata_from_rustup() {
            return Ok(path);
        }

        // Fall back to PATH
        let profdata_name = format!("llvm-profdata{}", std::env::consts::EXE_SUFFIX);
        if let Ok(output) = Command::new(&profdata_name).arg("--version").output()
            && output.status.success()
        {
            debug!("Found llvm-profdata in PATH");
            return Ok(PathBuf::from(profdata_name));
        }

        bail!(
            "Could not find `llvm-profdata`. Install it with:\n\
             \n  rustup component add llvm-tools\n"
        )
    }

    fn find_llvm_profdata_from_rustup() -> Result<PathBuf> {
        let sysroot_output = Command::new("rustc")
            .arg("--print")
            .arg("sysroot")
            .output()
            .context("Failed to run `rustc --print sysroot`")?;
        if !sysroot_output.status.success() {
            bail!("rustc --print sysroot failed");
        }
        let sysroot = std::str::from_utf8(&sysroot_output.stdout)
            .context("Invalid UTF-8 from rustc --print sysroot")?
            .trim();

        let verbose_output = Command::new("rustc")
            .arg("-vV")
            .output()
            .context("Failed to run `rustc -vV`")?;
        if !verbose_output.status.success() {
            bail!("rustc -vV failed");
        }
        let verbose =
            std::str::from_utf8(&verbose_output.stdout).context("Invalid UTF-8 from rustc -vV")?;
        let host = verbose
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .context("Could not determine host triple from `rustc -vV`")?;

        let profdata_name = format!("llvm-profdata{}", std::env::consts::EXE_SUFFIX);
        let profdata_path = PathBuf::from(sysroot)
            .join("lib")
            .join("rustlib")
            .join(host)
            .join("bin")
            .join(profdata_name);

        if profdata_path.exists() {
            debug!("Found llvm-profdata at {}", profdata_path.display());
            return Ok(profdata_path);
        }

        bail!("llvm-profdata not found at {}", profdata_path.display())
    }

    /// Run the PGO instrumentation workload.
    ///
    /// 1. Create a temporary venv (using `uv` when available, otherwise `python -m venv`)
    /// 2. Install the instrumented wheel
    /// 3. Install dependencies
    /// 4. Run the instrumentation command
    pub fn run_instrumentation(
        &self,
        python: &Path,
        wheel_path: &Path,
        build_context: &crate::BuildContext,
    ) -> Result<()> {
        let venv_dir = TempDir::new().context("Failed to create temporary venv directory")?;
        let venv_path = venv_dir.path();

        // Detect uv: try the binary first, then the Python module
        let uv = find_uv_python(python).or_else(|_| find_uv_bin()).ok();

        // Create venv
        if let Some((uv_path, uv_args)) = &uv {
            debug!("Creating venv with uv");
            let status = Command::new(uv_path)
                .args(uv_args.iter().copied())
                .args(["venv", "--python"])
                .arg(python)
                .arg(venv_path)
                .status()
                .context("Failed to create virtual environment with uv")?;
            if !status.success() {
                bail!("Failed to create virtual environment with uv (exit status: {status})");
            }
        } else {
            let status = Command::new(python)
                .args(["-m", "venv"])
                .arg(venv_path)
                .status()
                .context("Failed to create virtual environment")?;
            if !status.success() {
                bail!("Failed to create virtual environment (exit status: {status})");
            }
        }
        debug!("Created temporary venv at {}", venv_path.display());

        let venv_bin_dir = if cfg!(windows) {
            venv_path.join("Scripts")
        } else {
            venv_path.join("bin")
        };
        let venv_python = venv_bin_dir.join(if cfg!(windows) {
            "python.exe"
        } else {
            "python"
        });

        // Install the instrumented wheel
        eprintln!("📦 Installing instrumented wheel into temporary venv...");
        let status = self.pip_install(
            &uv,
            &venv_python,
            &["--force-reinstall", "--no-deps"],
            &[wheel_path],
        )?;
        if !status.success() {
            bail!("Failed to install instrumented wheel (exit status: {status})");
        }

        // Install requires_dist dependencies
        if !build_context.project.metadata24.requires_dist.is_empty() {
            debug!("Installing requires_dist dependencies");
            let deps: Vec<String> = build_context
                .project
                .metadata24
                .requires_dist
                .iter()
                .map(|x| x.to_string())
                .collect();
            let dep_refs: Vec<&Path> = deps.iter().map(|s| Path::new(s.as_str())).collect();
            let status = self.pip_install(&uv, &venv_python, &[], &dep_refs)?;
            if !status.success() {
                bail!("Failed to install dependencies (exit status: {status})");
            }
        }

        // Install dev dependency group if present (pip only — uv doesn't support --group yet)
        if uv.is_none() {
            let has_dev_group = build_context
                .project
                .pyproject_toml
                .as_ref()
                .and_then(|p| p.dependency_groups.as_ref())
                .is_some_and(|dg| dg.0.contains_key("dev"));
            if has_dev_group {
                let project_dir = build_context
                    .project
                    .pyproject_toml_path
                    .parent()
                    .context("Failed to get project directory")?;
                debug!("Installing dev dependency group");
                let status = Command::new(&venv_python)
                    .args(["-m", "pip", "install", "--group", "dev"])
                    .current_dir(project_dir)
                    .status()
                    .context("Failed to install dev dependency group")?;
                if !status.success() {
                    eprintln!(
                        "⚠️  Warning: failed to install dev dependency group \
                         (pip >= 25.1 required for --group support)"
                    );
                }
            }
        }

        eprintln!("🏃 Running instrumentation command: {}", self.pgo_command);
        let profraw_pattern = self
            .profdata_dir
            .path()
            .join("%m_%p.profraw")
            .to_string_lossy()
            .to_string();

        let current_path = std::env::var("PATH").unwrap_or_default();
        let sep = if cfg!(windows) { ";" } else { ":" };
        let path_env = format!("{}{sep}{current_path}", venv_bin_dir.display());

        let project_dir = build_context.project.project_layout.project_root.as_path();

        // Run through the system shell with the venv's bin dir prepended to PATH,
        // so that `python`, `pytest`, etc. resolve to the venv's copies.
        let mut cmd = if cfg!(windows) {
            let mut cmd = Command::new("cmd");
            cmd.args(["/C", &self.pgo_command]);
            cmd
        } else {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", &self.pgo_command]);
            cmd
        };

        cmd.current_dir(project_dir)
            .env("LLVM_PROFILE_FILE", &profraw_pattern)
            .env("PATH", &path_env)
            .env("VIRTUAL_ENV", venv_path);

        let status = cmd.status().with_context(|| {
            format!(
                "Failed to run PGO instrumentation command: {}",
                self.pgo_command
            )
        })?;
        if !status.success() {
            bail!(
                "PGO instrumentation command failed (exit status: {}): {}",
                status,
                self.pgo_command
            );
        }

        eprintln!("✅ PGO instrumentation completed successfully");
        Ok(())
    }

    /// Run `pip install` or `uv pip install` depending on what's available.
    fn pip_install(
        &self,
        uv: &Option<(PathBuf, Vec<&'static str>)>,
        venv_python: &Path,
        extra_args: &[&str],
        packages: &[&Path],
    ) -> Result<std::process::ExitStatus> {
        let status = if let Some((uv_path, uv_args)) = uv {
            Command::new(uv_path)
                .args(uv_args.iter().copied())
                .args(["pip", "install", "--python"])
                .arg(venv_python)
                .args(extra_args)
                .args(packages)
                .status()
                .context("Failed to run uv pip install")?
        } else {
            Command::new(venv_python)
                .args(["-m", "pip", "install"])
                .args(extra_args)
                .args(packages)
                .status()
                .context("Failed to run pip install")?
        };
        Ok(status)
    }

    /// Merge .profraw files into a single .profdata file
    pub fn merge_profiles(&self) -> Result<()> {
        eprintln!("🔗 Merging PGO profiles...");

        let llvm_profdata = Self::find_llvm_profdata()?;

        // Collect .profraw files, propagating any IO errors
        let profraws: Vec<PathBuf> = fs::read_dir(self.profdata_dir.path())
            .context("Failed to read profdata directory")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to read entry in profdata directory")?
            .into_iter()
            .map(|e| e.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "profraw"))
            .collect();

        if profraws.is_empty() {
            bail!(
                "PGO instrumentation completed but no .profraw files were generated.\n\
                 Make sure the instrumentation command exercises the compiled code."
            );
        }

        debug!("Found {} .profraw file(s) to merge", profraws.len());

        let status = Command::new(&llvm_profdata)
            .arg("merge")
            .arg("-o")
            .arg(&self.merged_profdata)
            .args(&profraws)
            .status()
            .with_context(|| format!("Failed to run `{} merge`", llvm_profdata.display()))?;
        if !status.success() {
            bail!("llvm-profdata merge failed (exit status: {})", status);
        }

        if !self.merged_profdata.exists() {
            bail!(
                "Merged profdata file not found at {}",
                self.merged_profdata.display()
            );
        }

        let metadata = fs::metadata(&self.merged_profdata)
            .context("Failed to read merged profdata metadata")?;
        debug!(
            "Merged profdata: {} ({} bytes)",
            self.merged_profdata.display(),
            metadata.len()
        );
        eprintln!("✅ Merged PGO profiles successfully");
        Ok(())
    }
}
