mod builder;

pub use builder::BuildContextBuilder;

#[cfg(feature = "sbom")]
use crate::auditwheel::get_sysroot_path;
use crate::auditwheel::{AuditWheelMode, get_policy_and_libs, patchelf, relpath};
use crate::auditwheel::{PlatformTag, Policy};
use crate::binding_generator::{
    BinBindingGenerator, BindingGenerator, CffiBindingGenerator, Pyo3BindingGenerator,
    UniFfiBindingGenerator, generate_binding,
};
use crate::bridge::Abi3Version;
use crate::build_options::CargoOptions;
use crate::compile::{CompileTarget, warn_missing_py_init};
use crate::compression::CompressionOptions;
#[cfg(feature = "sbom")]
use crate::module_writer::ModuleWriter;
use crate::module_writer::{WheelWriter, add_data, write_pth};
use crate::project_layout::ProjectLayout;
use crate::pyproject_toml::ConditionalFeature;
use crate::sbom::{SbomData, generate_sbom_data, write_sboms};
use crate::source_distribution::source_distribution;
use crate::target::validate_wheel_filename_for_pypi;
use crate::util::{hash_file, zip_mtime};
use crate::{
    BridgeModel, BuildArtifact, Metadata24, PyProjectToml, PythonInterpreter, Target,
    VirtualWriter, compile, pyproject_toml::Format, pyproject_toml::SbomConfig,
};
use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::CrateType;
use cargo_metadata::Metadata;
use fs_err as fs;
use ignore::overrides::{Override, OverrideBuilder};
use lddtree::Library;
use normpath::PathExt;
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tracing::instrument;

/// Contains all the metadata required to build the crate
#[derive(Clone)]
pub struct BuildContext {
    /// The platform, i.e. os and pointer width
    pub target: Target,
    /// List of Cargo targets to compile
    pub compile_targets: Vec<CompileTarget>,
    /// Whether this project is pure rust or rust mixed with python
    pub project_layout: ProjectLayout,
    /// The path to pyproject.toml. Required for the source distribution
    pub pyproject_toml_path: PathBuf,
    /// Parsed pyproject.toml if any
    pub pyproject_toml: Option<PyProjectToml>,
    /// Python Package Metadata 2.3
    pub metadata24: Metadata24,
    /// The name of the crate
    pub crate_name: String,
    /// The name of the module can be distinct from the package name, mostly
    /// because package names normally contain minuses while module names
    /// have underscores. The package name is part of metadata24
    pub module_name: String,
    /// The path to the Cargo.toml. Required for the cargo invocations
    pub manifest_path: PathBuf,
    /// Directory for all generated artifacts
    pub target_dir: PathBuf,
    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    pub out: PathBuf,
    /// Strip the library for minimum file size
    pub strip: bool,
    /// Checking the linked libraries for manylinux/musllinux compliance
    pub auditwheel: AuditWheelMode,
    /// When compiling for manylinux, use zig as linker to ensure glibc version compliance
    #[cfg(feature = "zig")]
    pub zig: bool,
    /// Whether to use the manylinux/musllinux or use the native linux tag (off)
    pub platform_tag: Vec<PlatformTag>,
    /// The available python interpreter
    pub interpreter: Vec<PythonInterpreter>,
    /// Cargo.toml as resolved by [cargo_metadata]
    pub cargo_metadata: Metadata,
    /// Whether to use universal2 or use the native macOS tag (off)
    pub universal2: bool,
    /// Build editable wheels
    pub editable: bool,
    /// Cargo build options
    pub cargo_options: CargoOptions,
    /// Compression options
    pub compression: CompressionOptions,
    /// Whether to validate wheels against PyPI platform tag rules
    pub pypi_validation: bool,
    /// SBOM configuration
    pub sbom: Option<SbomConfig>,
    /// Include the import library (.dll.lib) in the wheel on Windows
    pub include_import_lib: bool,
    /// Include debug info files (.pdb, .dSYM, .dwp) in the wheel
    pub include_debuginfo: bool,
    /// Cargo features conditionally enabled based on the target Python version/implementation
    pub conditional_features: Vec<ConditionalFeature>,
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
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the wheels")?;

        // Generate SBOM data once for all wheels (the Rust dependency graph
        // is the same regardless of the target Python interpreter).
        let sbom_data = generate_sbom_data(self)?;

        let wheels = match self.bridge() {
            BridgeModel::Bin(None) => self.build_bin_wheel(None, &sbom_data)?,
            BridgeModel::Bin(Some(..)) => self.build_bin_wheels(&self.interpreter, &sbom_data)?,
            BridgeModel::PyO3(crate::PyO3 { abi3, .. }) => match abi3 {
                Some(Abi3Version::Version(major, minor)) => {
                    self.build_abi3_wheels(Some((*major, *minor)), &sbom_data)?
                }
                Some(Abi3Version::CurrentPython) => self.build_abi3_wheels(None, &sbom_data)?,
                None => self.build_pyo3_wheels(&self.interpreter, &sbom_data)?,
            },
            BridgeModel::Cffi => self.build_cffi_wheel(&sbom_data)?,
            BridgeModel::UniFfi => self.build_uniffi_wheel(&sbom_data)?,
        };

        // Validate wheel filenames against PyPI platform tag rules if requested
        if self.pypi_validation {
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

    /// Split interpreters into abi3-capable and non-abi3 groups, build the
    /// appropriate wheel type for each group, and return all built wheels.
    ///
    /// When `min_version` is `Some((major, minor))` (i.e. `Abi3Version::Version`),
    /// interpreters below that version are excluded from the abi3 group.
    /// When `min_version` is `None` (i.e. `Abi3Version::CurrentPython`),
    /// all `has_stable_api()` interpreters are in the abi3 group and the
    /// baseline version is taken from the first one.
    fn build_abi3_wheels(
        &self,
        min_version: Option<(u8, u8)>,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        use itertools::Itertools;

        let abi3_interps: Vec<_> = self
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
            .interpreter
            .iter()
            .filter(|interp| {
                !interp.has_stable_api()
                    || min_version.is_some_and(|(major, minor)| {
                        (interp.major as u8, interp.minor as u8) < (major, minor)
                    })
            })
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

    /// Bridge model
    pub fn bridge(&self) -> &BridgeModel {
        // FIXME: currently we only allow multiple bin targets so bridges are all the same
        &self.compile_targets[0].bridge_model
    }

    /// Builds a source distribution and returns the same metadata as [BuildContext::build_wheels]
    pub fn build_source_distribution(&self) -> Result<Option<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the source distribution")?;

        match self.pyproject_toml.as_ref() {
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
                    let interp = &self.interpreter[0];
                    let platform = self.get_platform_tag(&[PlatformTag::Linux])?;
                    vec![format!(
                        "cp{major}{minor}-abi3-{platform}",
                        major = interp.major,
                        minor = interp.minor
                    )]
                }
                None => {
                    vec![self.interpreter[0].get_tag(self, &[PlatformTag::Linux])?]
                }
            },
            BridgeModel::Bin(None) | BridgeModel::Cffi | BridgeModel::UniFfi => {
                vec![self.get_universal_tag(&[PlatformTag::Linux])?]
            }
        };
        Ok(tags)
    }

    fn auditwheel(
        &self,
        artifact: &BuildArtifact,
        platform_tag: &[PlatformTag],
        python_interpreter: Option<&PythonInterpreter>,
    ) -> Result<(Policy, Vec<Library>)> {
        if matches!(self.auditwheel, AuditWheelMode::Skip) {
            return Ok((Policy::default(), Vec::new()));
        }

        if let Some(python_interpreter) = python_interpreter
            && platform_tag.is_empty()
            && self.target.is_linux()
            && !python_interpreter.support_portable_wheels()
        {
            eprintln!(
                "🐍 Skipping auditwheel because {python_interpreter} does not support manylinux/musllinux wheels"
            );
            return Ok((Policy::default(), Vec::new()));
        }

        let mut musllinux: Vec<_> = platform_tag
            .iter()
            .filter(|tag| tag.is_musllinux())
            .copied()
            .collect();
        musllinux.sort();
        let mut others: Vec<_> = platform_tag
            .iter()
            .filter(|tag| !tag.is_musllinux())
            .copied()
            .collect();
        others.sort();

        // only bin bindings allow linking to libpython, extension modules must not
        let allow_linking_libpython = self.bridge().is_bin();
        if self.bridge().is_bin() && !musllinux.is_empty() {
            return get_policy_and_libs(
                artifact,
                Some(musllinux[0]),
                &self.target,
                &self.manifest_path,
                allow_linking_libpython,
            );
        }

        let tag = others.first().or_else(|| musllinux.first()).copied();
        get_policy_and_libs(
            artifact,
            tag,
            &self.target,
            &self.manifest_path,
            allow_linking_libpython,
        )
    }

    /// Add library search paths in Cargo target directory rpath when building in editable mode
    fn add_rpath<A>(&self, artifacts: &[A]) -> Result<()>
    where
        A: Borrow<BuildArtifact>,
    {
        if self.editable && self.target.is_linux() && !artifacts.is_empty() {
            for artifact in artifacts {
                let artifact = artifact.borrow();
                if artifact.linked_paths.is_empty() {
                    continue;
                }
                let old_rpaths = patchelf::get_rpath(&artifact.path)?;
                let mut new_rpaths = old_rpaths.clone();
                for path in &artifact.linked_paths {
                    if !old_rpaths.contains(path) {
                        new_rpaths.push(path.to_string());
                    }
                }
                let new_rpath = new_rpaths.join(":");
                if let Err(err) = patchelf::set_rpath(&artifact.path, &new_rpath) {
                    eprintln!(
                        "⚠️ Warning: Failed to set rpath for {}: {}",
                        artifact.path.display(),
                        err
                    );
                }
            }
        }
        Ok(())
    }

    fn add_external_libs<A>(
        &self,
        writer: &mut VirtualWriter<WheelWriter>,
        artifacts: &[A],
        ext_libs: &[Vec<Library>],
    ) -> Result<()>
    where
        A: Borrow<BuildArtifact>,
    {
        if self.editable {
            return self.add_rpath(artifacts);
        }
        if ext_libs.iter().all(|libs| libs.is_empty()) {
            return Ok(());
        }

        // Log which libraries need to be copied and which artifacts require them
        // before calling patchelf, so users can see this even if patchelf is missing.
        eprintln!("🔗 External shared libraries to be copied into the wheel:");
        for (artifact, artifact_ext_libs) in artifacts.iter().zip(ext_libs) {
            let artifact = artifact.borrow();
            if artifact_ext_libs.is_empty() {
                continue;
            }
            eprintln!("  {} requires:", artifact.path.display());
            for lib in artifact_ext_libs {
                if let Some(path) = lib.realpath.as_ref() {
                    eprintln!("    {} => {}", lib.name, path.display());
                } else {
                    eprintln!("    {} => not found", lib.name);
                }
            }
        }

        if matches!(self.auditwheel, AuditWheelMode::Check) {
            bail!(
                "Your library is not manylinux/musllinux compliant because it requires copying the above libraries. \
                 Re-run with `--auditwheel=repair` to copy them."
            );
        }

        patchelf::verify_patchelf()?;

        // Put external libs to ${distribution_name}.libs directory
        // See https://github.com/pypa/auditwheel/issues/89
        // Use the distribution name (matching auditwheel's behavior) to avoid
        // conflicts with other packages in the same namespace.
        let libs_dir = PathBuf::from(format!(
            "{}.libs",
            self.metadata24.get_distribution_escaped()
        ));

        let temp_dir = writer.temp_dir()?;
        let mut soname_map = BTreeMap::new();
        let mut libs_copied = HashSet::new();
        for lib in ext_libs.iter().flatten() {
            let lib_path = lib.realpath.clone().with_context(|| {
                format!(
                    "Cannot repair wheel, because required library {} could not be located.",
                    lib.path.display()
                )
            })?;
            // Generate a new soname with a short hash
            let short_hash = &hash_file(&lib_path)?[..8];
            let (file_stem, file_ext) = lib.name.split_once('.').unwrap();
            let new_soname = if !file_stem.ends_with(&format!("-{short_hash}")) {
                format!("{file_stem}-{short_hash}.{file_ext}")
            } else {
                format!("{file_stem}.{file_ext}")
            };

            // Copy the original lib to a tmpdir and modify some of its properties
            // for example soname and rpath
            let dest_path = temp_dir.path().join(&new_soname);
            fs::copy(&lib_path, &dest_path)?;
            libs_copied.insert(lib_path);

            // fs::copy copies permissions as well, and the original
            // file may have been read-only
            let mut perms = fs::metadata(&dest_path)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            fs::set_permissions(&dest_path, perms)?;

            patchelf::set_soname(&dest_path, &new_soname)?;
            if !lib.rpath.is_empty() || !lib.runpath.is_empty() {
                patchelf::set_rpath(&dest_path, &libs_dir)?;
            }
            soname_map.insert(
                lib.name.clone(),
                (new_soname.clone(), dest_path.clone(), lib.needed.clone()),
            );
        }

        for (artifact, artifact_ext_libs) in artifacts.iter().zip(ext_libs) {
            let artifact = artifact.borrow();
            let artifact_deps: HashSet<_> = artifact_ext_libs.iter().map(|lib| &lib.name).collect();
            let replacements = soname_map
                .iter()
                .filter_map(|(k, v)| {
                    if artifact_deps.contains(k) {
                        Some((k, v.0.clone()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if !replacements.is_empty() {
                patchelf::replace_needed(&artifact.path, &replacements[..])?;
            }
        }

        // we grafted in a bunch of libraries and modified their sonames, but
        // they may have internal dependencies (DT_NEEDED) on one another, so
        // we need to update those records so each now knows about the new
        // name of the other.
        for (new_soname, path, needed) in soname_map.values() {
            let mut replacements = Vec::new();
            for n in needed {
                if soname_map.contains_key(n) {
                    replacements.push((n, soname_map[n].0.clone()));
                }
            }
            if !replacements.is_empty() {
                patchelf::replace_needed(path, &replacements[..])?;
            }
            // Use add_file_force to bypass exclusion checks for external shared libraries
            writer.add_file_force(libs_dir.join(new_soname), path, true)?;
        }

        // Sort for deterministic output.
        let mut grafted_paths: Vec<PathBuf> = libs_copied.into_iter().collect();
        grafted_paths.sort();

        eprintln!(
            "🖨  Copied external shared libraries to package {} directory.",
            libs_dir.display()
        );

        // Generate auditwheel SBOM for the grafted libraries.
        // This mirrors Python auditwheel's behaviour of writing a CycloneDX
        // SBOM to <dist-info>/sboms/auditwheel.cdx.json that records which OS
        // packages provided the grafted shared libraries.
        #[cfg(feature = "sbom")]
        {
            let auditwheel_sbom_enabled = self
                .sbom
                .as_ref()
                .and_then(|c| c.auditwheel)
                .unwrap_or(true);
            if auditwheel_sbom_enabled {
                // Obtain the sysroot so whichprovides can strip cross-compilation
                // prefixes when querying the host package manager.
                let sysroot = get_sysroot_path(&self.target).unwrap_or_else(|_| PathBuf::from("/"));
                if let Some(sbom_json) = crate::auditwheel::sbom::create_auditwheel_sbom(
                    &self.metadata24.name,
                    &self.metadata24.version.to_string(),
                    &grafted_paths,
                    &sysroot,
                ) {
                    let sbom_path = self
                        .metadata24
                        .get_dist_info_dir()
                        .join("sboms/auditwheel.cdx.json");
                    writer.add_bytes(&sbom_path, None, sbom_json, false)?;
                }
            }
        }

        let artifact_dir = match self.bridge() {
            // cffi bindings that contains '.' in the module name will be split into directories
            BridgeModel::Cffi => self.module_name.split(".").collect::<PathBuf>(),
            // For namespace packages the modules the modules resides resides at ${module_name}.so
            // where periods are replaced with slashes so for example my.namespace.module would reside
            // at my/namespace/module.so
            _ if self.module_name.contains(".") => {
                let mut path = self.module_name.split(".").collect::<PathBuf>();
                path.pop();
                path
            }
            // For other bindings artifact .so file usually resides at ${module_name}/${module_name}.so
            _ => PathBuf::from(&self.module_name),
        };
        for artifact in artifacts {
            let artifact = artifact.borrow();
            let mut new_rpaths = patchelf::get_rpath(&artifact.path)?;
            // TODO: clean existing rpath entries if it's not pointed to a location within the wheel
            // See https://github.com/pypa/auditwheel/blob/353c24250d66951d5ac7e60b97471a6da76c123f/src/auditwheel/repair.py#L160
            let new_rpath = Path::new("$ORIGIN").join(relpath(&libs_dir, &artifact_dir));
            new_rpaths.push(new_rpath.to_str().unwrap().to_string());
            let new_rpath = new_rpaths.join(":");
            patchelf::set_rpath(&artifact.path, &new_rpath)?;
        }
        Ok(())
    }

    fn add_pth(&self, writer: &mut VirtualWriter<WheelWriter>) -> Result<()> {
        if self.editable {
            write_pth(writer, &self.project_layout, &self.metadata24)?;
        }
        Ok(())
    }

    fn excludes(&self, format: Format) -> Result<Override> {
        let project_dir = match self.pyproject_toml_path.normalize() {
            Ok(pyproject_toml_path) => pyproject_toml_path.into_path_buf(),
            Err(_) => self.manifest_path.normalize()?.into_path_buf(),
        };
        let mut excludes = OverrideBuilder::new(project_dir.parent().unwrap());
        if let Some(pyproject) = self.pyproject_toml.as_ref()
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
                self.out.display(),
                std::path::MAIN_SEPARATOR,
                &self.metadata24.get_distribution_escaped(),
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
            self.pyproject_toml.as_ref(),
            &self.manifest_path,
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
        if self.platform_tag.is_empty() {
            vec![policy.platform_tag()]
        } else {
            self.platform_tag.clone()
        }
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
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let writer = WheelWriter::new(tag, &self.out, &self.metadata24, file_options)?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);
        self.add_external_libs(&mut writer, artifacts, ext_libs)?;

        let temp_dir = writer.temp_dir()?;
        let mut generator = make_generator(temp_dir)?;
        generate_binding(&mut writer, generator.as_mut(), self, artifacts, out_dirs)
            .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &self.metadata24,
            self.project_layout.data.as_deref(),
        )?;

        write_sboms(
            self,
            sbom_data.as_ref(),
            &mut writer,
            &self.metadata24.get_dist_info_dir(),
        )?;

        let tags = [tag.to_string()];
        let wheel_path =
            writer.finish(&self.metadata24, &self.project_layout.project_root, &tags)?;
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
            Some(&self.project_layout.extension_name),
        )?;
        let (policy, external_libs) =
            self.auditwheel(&artifact, &self.platform_tag, python_interpreter)?;
        let platform_tags = self.resolve_platform_tags(&policy);

        let platform = self.get_platform_tag(&platform_tags)?;
        let tag = format!("cp{major}{min_minor}-abi3-{platform}");

        let wheel_path = self.write_wheel(
            &tag,
            &[&artifact],
            &[external_libs],
            |temp_dir| {
                Ok(Box::new(
                    Pyo3BindingGenerator::new(true, self.interpreter.first(), temp_dir)
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
        let tag = python_interpreter.get_tag(self, platform_tags)?;

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
            let (artifact, out_dirs) = self.compile_cdylib(
                Some(python_interpreter),
                Some(&self.project_layout.extension_name),
            )?;
            let (policy, external_libs) =
                self.auditwheel(&artifact, &self.platform_tag, Some(python_interpreter))?;
            let platform_tags = self.resolve_platform_tags(&policy);
            let wheel_path = self.write_pyo3_wheel(
                python_interpreter,
                artifact,
                &platform_tags,
                external_libs,
                sbom_data,
                &out_dirs,
            )?;
            eprintln!(
                "📦 Built wheel for {} {}.{}{} to {}",
                python_interpreter.interpreter_kind,
                python_interpreter.major,
                python_interpreter.minor,
                python_interpreter.abiflags,
                wheel_path.display()
            );

            let tag = format!("cp{}{}", python_interpreter.major, python_interpreter.minor);
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
        let result = compile(self, python_interpreter, &self.compile_targets)
            .context("Failed to build a native library through cargo")?;
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

    /// Stage an artifact into a private directory so that:
    /// 1. `warn_missing_py_init` can safely mmap the file without risk of
    ///    concurrent modification by cargo / rust-analyzer.
    /// 2. Auditwheel repair can modify it in-place without altering the
    ///    original cargo build output.
    ///
    /// Uses `fs::rename` for an atomic move into the staging directory,
    /// then copies the staged file back to the original location so that
    /// users can still find the artifact at the standard cargo output
    /// path. The copy-back uses reflink (copy-on-write) when available
    /// for near-instant, zero-cost copies, and falls back to a regular
    /// `fs::copy` otherwise.
    ///
    /// When `fs::rename` fails (e.g. cross-device), falls back to
    /// reflink-or-copy directly; the concurrent-modification window is
    /// unlikely in cross-device setups.
    fn stage_artifact(&self, artifact: &mut BuildArtifact) -> Result<()> {
        let maturin_build = self.target_dir.join(env!("CARGO_PKG_NAME"));
        fs::create_dir_all(&maturin_build)?;
        let artifact_path = &artifact.path;
        let new_artifact_path = maturin_build.join(artifact_path.file_name().unwrap());
        // Remove any stale file at the destination so that `fs::rename`
        // succeeds on Windows (where rename fails if the destination
        // already exists).
        let _ = fs::remove_file(&new_artifact_path);
        if fs::rename(artifact_path, &new_artifact_path).is_ok() {
            // Rename succeeded — we now own the only copy.  Put a copy
            // back at the original location for users who expect the
            // artifact at the standard cargo output path.  Skip if a
            // new file already appeared (cargo / rust-analyzer rebuilt).
            if artifact_path.exists() {
                tracing::debug!(
                    "Skipping copy-back: {} was recreated by another process",
                    artifact_path.display()
                );
            } else if let Err(err) = reflink_or_copy(&new_artifact_path, artifact_path) {
                eprintln!(
                    "⚠️  Warning: failed to copy artifact back to {}: {err:#}. The staged artifact is available at {}",
                    artifact_path.display(),
                    new_artifact_path.display()
                );
            }
        } else {
            // Rename failed (cross-device).  Fall back to reflink/copy;
            // concurrent modification is unlikely in this scenario.
            reflink_or_copy(artifact_path, &new_artifact_path)?;
        }
        artifact.path = new_artifact_path.normalize()?.into_path_buf();
        Ok(())
    }

    fn write_cffi_wheel(
        &self,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<PathBuf> {
        let tag = self.get_universal_tag(platform_tags)?;

        let interpreter = self.interpreter.first().ok_or_else(|| {
            anyhow!("A python interpreter is required for cffi builds but one was not provided")
        })?;
        self.write_wheel(
            &tag,
            &[&artifact],
            &[ext_libs],
            |temp_dir| {
                Ok(Box::new(
                    CffiBindingGenerator::new(interpreter, temp_dir)
                        .context("Failed to initialize Cffi binding generator")?,
                ))
            },
            sbom_data,
            out_dirs,
        )
    }

    /// Builds a wheel with cffi bindings
    pub fn build_cffi_wheel(
        &self,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        let (artifact, out_dirs) = self.compile_cdylib(None, None)?;
        let (policy, external_libs) = self.auditwheel(&artifact, &self.platform_tag, None)?;
        let platform_tags = self.resolve_platform_tags(&policy);
        let wheel_path = self.write_cffi_wheel(
            artifact,
            &platform_tags,
            external_libs,
            sbom_data,
            &out_dirs,
        )?;

        // Warn if cffi isn't specified in the requirements
        if !self
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
        wheels.push((wheel_path, "py3".to_string()));

        Ok(wheels)
    }

    fn write_uniffi_wheel(
        &self,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
        sbom_data: &Option<SbomData>,
        out_dirs: &HashMap<String, PathBuf>,
    ) -> Result<PathBuf> {
        let tag = self.get_universal_tag(platform_tags)?;

        self.write_wheel(
            &tag,
            &[&artifact],
            &[ext_libs],
            |_temp_dir| Ok(Box::new(UniFfiBindingGenerator::default())),
            sbom_data,
            out_dirs,
        )
    }

    /// Builds a wheel with uniffi bindings
    pub fn build_uniffi_wheel(
        &self,
        sbom_data: &Option<SbomData>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        let (artifact, out_dirs) = self.compile_cdylib(None, None)?;
        let (policy, external_libs) = self.auditwheel(&artifact, &self.platform_tag, None)?;
        let platform_tags = self.resolve_platform_tags(&policy);
        let wheel_path = self.write_uniffi_wheel(
            artifact,
            &platform_tags,
            external_libs,
            sbom_data,
            &out_dirs,
        )?;

        eprintln!("📦 Built wheel to {}", wheel_path.display());
        wheels.push((wheel_path, "py3".to_string()));

        Ok(wheels)
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
        if !self.metadata24.scripts.is_empty() {
            bail!("Defining scripts and working with a binary doesn't mix well");
        }

        if self.target.is_wasi() {
            eprintln!("⚠️  Warning: wasi support is experimental");
            if !self.metadata24.entry_points.is_empty() {
                bail!("You can't define entrypoints yourself for a binary project");
            }

            if self.project_layout.python_module.is_some() {
                // TODO: Can we have python code and the wasm launchers coexisting
                // without clashes?
                bail!("Sorry, adding python code to a wasm binary is currently not supported")
            }
        }

        let tag = match (self.bridge(), python_interpreter) {
            (BridgeModel::Bin(None), _) => self.get_universal_tag(platform_tags)?,
            (BridgeModel::Bin(Some(..)), Some(python_interpreter)) => {
                python_interpreter.get_tag(self, platform_tags)?
            }
            _ => unreachable!(),
        };

        let mut metadata24 = self.metadata24.clone();
        let file_options = self
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let writer = WheelWriter::new(&tag, &self.out, &metadata24, file_options)?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);

        self.add_external_libs(&mut writer, artifacts, ext_libs)?;

        let mut generator = BinBindingGenerator::new(&mut metadata24);
        generate_binding(&mut writer, &mut generator, self, artifacts, out_dirs)
            .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &metadata24,
            self.project_layout.data.as_deref(),
        )?;
        write_sboms(
            self,
            sbom_data.as_ref(),
            &mut writer,
            &metadata24.get_dist_info_dir(),
        )?;
        let tags = [tag];
        let wheel_path = writer.finish(&metadata24, &self.project_layout.project_root, &tags)?;
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
        let result = compile(self, python_interpreter, &self.compile_targets)
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

            let (policy, external_libs) = self.auditwheel(&artifact, &self.platform_tag, None)?;
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

/// Reflink (copy-on-write) a file, preserving permissions, and fall back to
/// a regular copy if reflink fails for any reason.
///
/// On macOS `clonefile` preserves all metadata natively.  On Linux
/// `ioctl_ficlone` only clones data blocks so we must copy permissions
/// ourselves.
///
/// Adapted from uv's `reflink_with_permissions` implementation:
/// <https://github.com/astral-sh/uv/blob/main/crates/uv-fs/src/link.rs>
/// See also: <https://github.com/astral-sh/uv/issues/18181>
fn reflink_or_copy(from: &Path, to: &Path) -> Result<()> {
    if reflink_with_permissions(from, to).is_err() {
        fs::copy(from, to)?;
    }
    Ok(())
}

/// Attempt a reflink while preserving the source file's permissions.
///
/// On Linux, `ioctl_ficlone` does not copy metadata, so we reflink first
/// then copy permissions from the source to the destination.
/// On other platforms we delegate to `reflink_copy::reflink` which preserves
/// metadata natively (macOS `clonefile`).
///
/// Based on uv's approach which uses `rustix::fs::ioctl_ficlone` directly
/// with `fchmod` on the open file descriptor to avoid TOCTOU races.  We
/// simplify here by calling `reflink_copy::reflink` followed by
/// `set_permissions`, since the staged artifact lives in a private
/// directory where TOCTOU is not a concern.
/// <https://github.com/astral-sh/uv/blob/main/crates/uv-fs/src/link.rs>
#[cfg(target_os = "linux")]
fn reflink_with_permissions(from: &Path, to: &Path) -> std::io::Result<()> {
    reflink_copy::reflink(from, to)?;
    let perms = fs::metadata(from)?.permissions();
    fs::set_permissions(to, perms)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn reflink_with_permissions(from: &Path, to: &Path) -> std::io::Result<()> {
    reflink_copy::reflink(from, to)
}
