use crate::auditwheel::{AuditWheelMode, PlatformTag};
use crate::bridge::{find_bridge, has_windows_import_lib_support, upgrade_bridge_abi3};
use crate::build_options::{BuildOptions, TargetTriple};
use crate::compile::filter_cargo_targets;
use crate::metadata::Metadata24;
use crate::project_layout::ProjectResolver;
use crate::pyproject_toml::{FeatureSpec, SbomConfig};
use crate::python_interpreter::{InterpreterResolver, PythonInterpreter};
use crate::target::{
    detect_arch_from_python, detect_target_from_cross_python, is_arch_supported_by_pypi,
};
use crate::{
    ArtifactContext, BridgeModel, BuildContext, ProjectContext, PyProjectToml, PythonContext,
    Target,
};
use anyhow::{Result, bail};
use std::collections::HashSet;
use std::env;
use std::path::PathBuf;
use tracing::{debug, instrument};

/// Builder for constructing a [`BuildContext`] from [`BuildOptions`].
///
/// Created via [`BuildOptions::into_build_context()`], then configured
/// with chained setter methods before calling [`build()`](Self::build).
#[derive(Debug)]
pub struct BuildContextBuilder {
    build_options: BuildOptions,
    strip: Option<bool>,
    editable: bool,
    sdist_only: bool,
    pyproject_toml_path: Option<PathBuf>,
    pgo: bool,
}

impl BuildContextBuilder {
    pub(crate) fn new(build_options: BuildOptions) -> Self {
        Self {
            build_options,
            strip: None,
            editable: false,
            sdist_only: false,
            pyproject_toml_path: None,
            pgo: false,
        }
    }

    pub fn strip(mut self, strip: Option<bool>) -> Self {
        self.strip = strip;
        self
    }

    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    pub fn sdist_only(mut self, sdist_only: bool) -> Self {
        self.sdist_only = sdist_only;
        self
    }

    pub fn pyproject_toml_path(mut self, path: Option<PathBuf>) -> Self {
        self.pyproject_toml_path = path;
        self
    }

    pub fn pgo(mut self, pgo: bool) -> Self {
        self.pgo = pgo;
        self
    }

    #[instrument(skip_all)]
    pub fn build(self) -> Result<BuildContext> {
        let Self {
            build_options,
            strip,
            editable,
            sdist_only,
            pyproject_toml_path: explicit_pyproject_path,
            pgo,
        } = self;
        build_options.compression.validate();
        let ProjectResolver {
            project_layout,
            cargo_toml_path,
            cargo_toml,
            pyproject_toml_path,
            pyproject_toml,
            module_name,
            metadata24,
            mut cargo_options,
            cargo_metadata,
            mut pyproject_toml_maturin_options,
        } = ProjectResolver::resolve(
            build_options.manifest_path.clone(),
            build_options.cargo.clone(),
            editable,
            explicit_pyproject_path,
        )?;
        let pyproject = pyproject_toml.as_ref();

        let bindings = build_options.python.bindings.as_deref().or_else(|| {
            pyproject.and_then(|x| {
                if x.bindings().is_some() {
                    pyproject_toml_maturin_options.push("bindings");
                }
                x.bindings()
            })
        });

        // Check whether conditional pyo3/pyo3-ffi features exist in pyproject.toml
        // AND pyproject features are actually active (not overridden by CLI --features).
        // When CLI --features is set, pyproject features are ignored at compile time
        // (see cargo_options.merge_with_pyproject_toml), so bridge inference must
        // ignore them too to stay in sync.
        let has_conditional_pyo3_features = pyproject
            .and_then(|p| p.maturin())
            .and_then(|m| m.features.as_ref())
            .is_some_and(|specs| {
                // Only consider conditional features when pyproject features
                // were actually adopted (not overridden by CLI).
                let cli_overrides = !cargo_options.features.is_empty()
                    && !pyproject_toml_maturin_options.contains(&"features");
                !cli_overrides
                    && FeatureSpec::split(specs.clone()).1.iter().any(|c| {
                        c.feature.starts_with("pyo3/") || c.feature.starts_with("pyo3-ffi/")
                    })
            });

        // Detect bridge without conditional pyo3 features — those are
        // evaluated after interpreter resolution via upgrade_bridge_abi3.
        let bridge = find_bridge(&cargo_metadata, bindings)?;

        if !bridge.is_bin() && project_layout.extension_name.contains('-') {
            bail!(
                "The module name must not contain a minus `-` \
                 (Make sure you have set an appropriate [lib] name or \
                 [tool.maturin] module-name in your pyproject.toml)"
            );
        }

        let (target, universal2) = resolve_target(
            build_options.target.clone(),
            build_options.python.interpreter.first(),
        )?;

        let wheel_dir = match build_options.output.out {
            Some(ref dir) => dir.clone(),
            None => PathBuf::from(&cargo_metadata.target_directory).join("wheels"),
        };

        let (interpreter, host_python) = Self::resolve_interpreters(
            &build_options,
            sdist_only,
            &target,
            &bridge,
            &metadata24,
            &cargo_metadata,
        )?;

        // Upgrade bridge to abi3 if conditional pyo3 features
        // (e.g. abi3-py311 gated on python-version>=3.11) match any
        // of the resolved interpreters.
        let bridge = if has_conditional_pyo3_features {
            upgrade_bridge_abi3(bridge, &cargo_metadata, pyproject, &interpreter)?
        } else {
            bridge
        };
        debug!("Resolved bridge model: {:?}", bridge);
        if let Some(stable_abi) = bridge.pyo3().and_then(|p| p.stable_abi.as_ref()) {
            eprintln!(
                "🔗 Found {bridge} bindings with {} support",
                stable_abi.kind
            );
        } else {
            eprintln!("🔗 Found {bridge} bindings");
        }

        // Set PYO3_PYTHON for cross-compilation so pyo3's build script
        // can find the host interpreter.
        if let Some(ref host_python) = host_python {
            unsafe {
                env::set_var("PYO3_PYTHON", host_python);
                env::set_var("PYTHON_SYS_EXECUTABLE", host_python);
            }
        }

        if cargo_options.args.is_empty() {
            // if not supplied on command line, try pyproject.toml
            let tool_maturin = pyproject.and_then(|p| p.maturin());
            if let Some(args) = tool_maturin.and_then(|x| x.rustc_args.as_ref()) {
                cargo_options.args.extend(args.iter().cloned());
                pyproject_toml_maturin_options.push("rustc-args");
            }
        }

        let (strip, include_debuginfo, auditwheel) =
            Self::resolve_build_flags(strip, &build_options, pyproject, &target);

        let sbom = Self::resolve_sbom_config(&build_options, pyproject);

        // Check if PyPI validation is needed from the original user input,
        // since resolve_platform_tags filters out PlatformTag::Pypi
        let pypi_validation = build_options
            .platform
            .platform_tag
            .iter()
            .any(|platform_tag| platform_tag == &PlatformTag::Pypi);

        let platform_tags = resolve_platform_tags(
            build_options.platform.platform_tag,
            &target,
            &bridge,
            pyproject,
            &mut pyproject_toml_maturin_options,
            #[cfg(feature = "zig")]
            build_options.platform.zig,
        )?;

        validate_bridge_type(&bridge, &target, &platform_tags)?;

        // linux tag can not be mixed with manylinux and musllinux tags
        if platform_tags.len() > 1 && platform_tags.iter().any(|tag| !tag.is_portable()) {
            bail!("Cannot mix linux and manylinux/musllinux platform tags",);
        }

        if !pyproject_toml_maturin_options.is_empty() {
            eprintln!(
                "📡 Using build options {} from pyproject.toml",
                pyproject_toml_maturin_options.join(", ")
            );
        }

        let target_dir = build_options
            .cargo
            .target_dir
            .clone()
            .unwrap_or_else(|| cargo_metadata.target_directory.clone().into_std_path_buf());

        let config_targets = pyproject.and_then(|x| x.targets());
        let compile_targets = filter_cargo_targets(&cargo_metadata, bridge, config_targets)?;
        if compile_targets.is_empty() {
            bail!(
                "No Cargo targets to build, please check your bindings configuration in pyproject.toml."
            );
        }

        let crate_name = cargo_toml.package.name;
        let include_import_lib = pyproject
            .map(|p| p.include_import_lib())
            .unwrap_or_default();
        let conditional_features = if pyproject_toml_maturin_options.contains(&"features") {
            pyproject_toml
                .as_ref()
                .and_then(|p| p.maturin())
                .and_then(|m| m.features.clone())
                .map(|specs| FeatureSpec::split(specs).1)
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let pgo_command = if pgo {
            let cmd = pyproject
                .and_then(|p| p.pgo_command())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            if cmd.is_none() {
                bail!(
                    "--pgo requires a non-empty `pgo-command` to be set in `[tool.maturin]` in pyproject.toml"
                );
            }
            cmd
        } else {
            None
        };

        Ok(BuildContext {
            project: ProjectContext {
                target,
                project_layout,
                pyproject_toml_path,
                pyproject_toml,
                metadata24,
                crate_name,
                module_name,
                manifest_path: cargo_toml_path,
                target_dir,
                cargo_metadata,
                universal2,
                editable,
                cargo_options,
                conditional_features,
                compile_targets,
            },
            artifact: ArtifactContext {
                out: wheel_dir,
                strip,
                compression: build_options.compression,
                sbom,
                include_import_lib,
                include_debuginfo,
                pgo_phase: None,
                pgo_command,
                generate_stubs: build_options.generate_stubs,
            },
            python: PythonContext {
                auditwheel,
                #[cfg(feature = "zig")]
                zig: build_options.platform.zig,
                platform_tag: platform_tags,
                interpreter,
                pypi_validation,
            },
        })
    }

    /// Resolve Python interpreters for the build.
    fn resolve_interpreters(
        build_options: &BuildOptions,
        sdist_only: bool,
        target: &Target,
        bridge: &BridgeModel,
        metadata24: &Metadata24,
        cargo_metadata: &cargo_metadata::Metadata,
    ) -> Result<(Vec<PythonInterpreter>, Option<PathBuf>)> {
        let has_import_lib_support = has_windows_import_lib_support(cargo_metadata)?;
        if sdist_only && env::var_os("MATURIN_TEST_PYTHON").is_none() {
            return Ok((Vec::new(), None));
        }

        let mut user_interpreters = build_options.python.interpreter.clone();
        if cfg!(test)
            && user_interpreters.is_empty()
            && !build_options.python.find_interpreter
            && let Some(python) = env::var_os("MATURIN_TEST_PYTHON")
        {
            user_interpreters = vec![python.into()];
        }

        let resolver = InterpreterResolver::new(
            target,
            bridge,
            metadata24.requires_python.as_ref(),
            &user_interpreters,
            build_options.python.find_interpreter,
            has_import_lib_support,
        );
        resolver.resolve()
    }

    /// Resolve strip, debuginfo, and auditwheel mode from CLI + pyproject.toml.
    fn resolve_build_flags(
        strip: Option<bool>,
        build_options: &BuildOptions,
        pyproject: Option<&PyProjectToml>,
        target: &Target,
    ) -> (bool, bool, AuditWheelMode) {
        let strip = strip.unwrap_or_else(|| pyproject.map(|x| x.strip()).unwrap_or_default());
        let include_debuginfo = if strip && build_options.output.include_debuginfo {
            tracing::warn!("--strip is enabled, disabling --include-debuginfo");
            false
        } else if strip {
            false
        } else {
            build_options.output.include_debuginfo
        };
        let skip_auditwheel = pyproject.map(|x| x.skip_auditwheel()).unwrap_or_default()
            || build_options.platform.skip_auditwheel;
        let default_mode = if skip_auditwheel {
            AuditWheelMode::Skip
        } else if target.is_linux() {
            AuditWheelMode::Repair
        } else {
            // macOS and Windows repair support is newer;
            // default to Warn so we don't break existing workflows.
            AuditWheelMode::Warn
        };
        let auditwheel = build_options
            .platform
            .auditwheel
            .or_else(|| pyproject.and_then(|x| x.auditwheel()))
            .unwrap_or(default_mode);
        (strip, include_debuginfo, auditwheel)
    }

    /// Resolve SBOM configuration from CLI + pyproject.toml.
    fn resolve_sbom_config(
        build_options: &BuildOptions,
        pyproject: Option<&PyProjectToml>,
    ) -> Option<SbomConfig> {
        let mut config = pyproject
            .and_then(|x| x.maturin())
            .and_then(|x| x.sbom.clone())
            .unwrap_or_default();
        if !build_options.output.sbom_include.is_empty() {
            let includes = config.include.get_or_insert_with(Vec::new);
            includes.extend(build_options.output.sbom_include.iter().cloned());
            includes.dedup();
        }
        Some(config)
    }
}

/// Resolve the build target and universal2 flag from the user-specified
/// target triple (or `ARCHFLAGS`) and the first interpreter (if any).
fn resolve_target(
    target_triple: Option<TargetTriple>,
    first_interpreter: Option<&PathBuf>,
) -> Result<(Target, bool)> {
    let mut target_triple = target_triple;
    let mut universal2 = target_triple == Some(TargetTriple::Universal2);

    // Also try to determine universal2 from ARCHFLAGS environment variable
    if target_triple.is_none()
        && let Ok(arch_flags) = env::var("ARCHFLAGS")
    {
        let arches: HashSet<&str> = arch_flags
            .split("-arch")
            .filter_map(|x| {
                let x = x.trim();
                if x.is_empty() { None } else { Some(x) }
            })
            .collect();
        match (arches.contains("x86_64"), arches.contains("arm64")) {
            (true, true) => universal2 = true,
            (true, false) => {
                target_triple = Some(TargetTriple::Regular("x86_64-apple-darwin".to_string()))
            }
            (false, true) => {
                target_triple = Some(TargetTriple::Regular("aarch64-apple-darwin".to_string()))
            }
            (false, false) => {}
        }
    };
    if universal2 {
        target_triple = Some(TargetTriple::Regular("aarch64-apple-darwin".to_string()));
    }

    let mut target = Target::from_target_triple(target_triple.as_ref())?;
    if !target.user_specified && !universal2 {
        if let Some(interpreter) = first_interpreter {
            if let Some(detected_target) = detect_target_from_cross_python(interpreter) {
                target = Target::from_target_triple(Some(&detected_target))?;
            } else if let Some(detected_target) = detect_arch_from_python(interpreter, &target) {
                target = Target::from_target_triple(Some(&detected_target))?;
            }
        } else if let Some(detected_target) = detect_target_from_cross_python(&target.get_python())
        {
            target = Target::from_target_triple(Some(&detected_target))?;
        }
    }

    Ok((target, universal2))
}

/// Resolve platform tags from CLI flags, pyproject.toml, and target properties.
fn resolve_platform_tags(
    user_tags: Vec<PlatformTag>,
    target: &Target,
    bridge: &BridgeModel,
    pyproject: Option<&crate::pyproject_toml::PyProjectToml>,
    pyproject_options: &mut Vec<&str>,
    #[cfg(feature = "zig")] use_zig: bool,
) -> Result<Vec<PlatformTag>> {
    let platform_tags = if user_tags.is_empty() {
        #[cfg(feature = "zig")]
        let zig = use_zig;
        #[cfg(not(feature = "zig"))]
        let zig = false;
        let compatibility = pyproject
            .and_then(|x| {
                if x.compatibility().is_some() {
                    pyproject_options.push("compatibility");
                }
                x.compatibility()
            })
            .or(if zig {
                if target.is_musl_libc() {
                    Some(PlatformTag::Musllinux { major: 1, minor: 2 })
                } else {
                    Some(target.get_minimum_manylinux_tag())
                }
            } else if target.is_musl_libc() && !bridge.is_bin() {
                Some(PlatformTag::Musllinux { major: 1, minor: 2 })
            } else {
                None
            });
        if let Some(platform_tag) = compatibility {
            vec![platform_tag]
        } else {
            Vec::new()
        }
    } else if let [PlatformTag::Pypi] = &user_tags[..] {
        if !is_arch_supported_by_pypi(target) {
            bail!("Rust target {target} is not supported by PyPI");
        }
        Vec::new()
    } else {
        if user_tags.iter().any(|tag| tag.is_pypi()) && !is_arch_supported_by_pypi(target) {
            bail!("Rust target {target} is not supported by PyPI");
        }
        user_tags
            .into_iter()
            .filter(|platform_tag| platform_tag != &PlatformTag::Pypi)
            .collect()
    };

    for platform_tag in &platform_tags {
        if !platform_tag.is_supported() {
            eprintln!("⚠️  Warning: {platform_tag} is unsupported by the Rust compiler.");
        } else if platform_tag.is_musllinux() && !target.is_musl_libc() {
            eprintln!("⚠️  Warning: {target} is not compatible with {platform_tag}.");
        }
    }

    Ok(platform_tags)
}

/// Checks for bridge/platform type edge cases
fn validate_bridge_type(
    bridge: &BridgeModel,
    target: &Target,
    platform_tags: &[PlatformTag],
) -> Result<()> {
    match bridge {
        BridgeModel::Bin(None) => {
            // Only support two different kind of platform tags when compiling to musl target without any binding crates
            if platform_tags.iter().any(|tag| tag.is_musllinux()) && !target.is_musl_libc() {
                bail!(
                    "Cannot mix musllinux and manylinux platform tags when compiling to {}",
                    target.target_triple()
                );
            }

            if platform_tags.len() > 2 {
                bail!(
                    "Expected only one or two platform tags but found {}",
                    platform_tags.len()
                );
            } else if platform_tags.len() == 2 {
                // The two platform tags can't be the same kind
                let tag_types = platform_tags
                    .iter()
                    .map(|tag| tag.is_musllinux())
                    .collect::<HashSet<_>>();
                if tag_types.len() == 1 {
                    bail!(
                        "Expected only one platform tag but found {}",
                        platform_tags.len()
                    );
                }
            }
        }
        _ => {
            if platform_tags.len() > 1 {
                bail!(
                    "Expected only one platform tag but found {}",
                    platform_tags.len()
                );
            }
        }
    }
    Ok(())
}
