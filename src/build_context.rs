use crate::auditwheel::{get_policy_and_libs, patchelf, relpath};
use crate::auditwheel::{PlatformTag, Policy};
use crate::build_options::CargoOptions;
use crate::compile::{warn_missing_py_init, CompileTarget};
use crate::module_writer::{
    add_data, write_bin, write_bindings_module, write_cffi_module, write_python_part,
    write_uniffi_module, write_wasm_launcher, WheelWriter,
};
use crate::project_layout::ProjectLayout;
use crate::python_interpreter::InterpreterKind;
use crate::source_distribution::source_distribution;
use crate::{
    compile, pyproject_toml::Format, BuildArtifact, Metadata21, ModuleWriter, PyProjectToml,
    PythonInterpreter, Target,
};
use anyhow::{anyhow, bail, Context, Result};
use cargo_metadata::Metadata;
use fs_err as fs;
use ignore::overrides::{Override, OverrideBuilder};
use lddtree::Library;
use normpath::PathExt;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::io;
use std::path::{Path, PathBuf};

/// The way the rust code is used in the wheel
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeModel {
    /// A rust binary to be shipped a python package
    /// The String is the name of the bindings
    /// providing crate, e.g. pyo3, the number is the minimum minor python version
    Bin(Option<(String, usize)>),
    /// A native module with pyo3 or rust-cpython bindings. The String is the name of the bindings
    /// providing crate, e.g. pyo3, the number is the minimum minor python version
    Bindings(String, usize),
    /// `Bindings`, but specifically for pyo3 with feature flags that allow building a single wheel
    /// for all cpython versions (pypy still needs multiple versions).
    /// The numbers are the minimum major and minor version
    BindingsAbi3(u8, u8),
    /// A native module with c bindings, i.e. `#[no_mangle] extern "C" <some item>`
    Cffi,
    /// A native module generated from uniffi
    UniFfi,
}

impl BridgeModel {
    /// Returns the name of the bindings crate
    pub fn unwrap_bindings(&self) -> &str {
        match self {
            BridgeModel::Bindings(value, _) => value,
            _ => panic!("Expected Bindings"),
        }
    }

    /// Test whether this is using a specific bindings crate
    pub fn is_bindings(&self, name: &str) -> bool {
        match self {
            BridgeModel::Bin(Some((value, _))) => value == name,
            BridgeModel::Bindings(value, _) => value == name,
            _ => false,
        }
    }

    /// Test whether this is bin bindings
    pub fn is_bin(&self) -> bool {
        matches!(self, BridgeModel::Bin(_))
    }
}

impl Display for BridgeModel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeModel::Bin(Some((name, _))) => write!(f, "{name} bin"),
            BridgeModel::Bin(None) => write!(f, "bin"),
            BridgeModel::Bindings(name, _) => write!(f, "{name}"),
            BridgeModel::BindingsAbi3(..) => write!(f, "pyo3"),
            BridgeModel::Cffi => write!(f, "cffi"),
            BridgeModel::UniFfi => write!(f, "uniffi"),
        }
    }
}

/// Insert wasm launcher scripts as entrypoints and the wasmtime dependency
fn bin_wasi_helper(
    artifacts_and_files: &[(&BuildArtifact, String)],
    mut metadata21: Metadata21,
) -> Result<Metadata21> {
    eprintln!("⚠️  Warning: wasi support is experimental");
    // escaped can contain [\w\d.], but i don't know how we'd handle dots correctly here
    if metadata21.get_distribution_escaped().contains('.') {
        bail!(
            "Can't build wasm wheel if there is a dot in the name ('{}')",
            metadata21.get_distribution_escaped()
        )
    }
    if !metadata21.entry_points.is_empty() {
        bail!("You can't define entrypoints yourself for a binary project");
    }

    let mut console_scripts = HashMap::new();
    for (_, bin_name) in artifacts_and_files {
        let base_name = bin_name
            .strip_suffix(".wasm")
            .context("No .wasm suffix in wasi binary")?;
        console_scripts.insert(
            base_name.to_string(),
            format!(
                "{}.{}:main",
                metadata21.get_distribution_escaped(),
                base_name.replace('-', "_")
            ),
        );
    }

    metadata21
        .entry_points
        .insert("console_scripts".to_string(), console_scripts);

    // A real pip version specification parser would be better, but bearing this we use this regex
    // which tries to find the name wasmtime and then any specification
    let wasmtime_re = Regex::new("^wasmtime[^a-zA-Z.-_]").unwrap();
    if !metadata21
        .requires_dist
        .iter()
        .any(|requirement| wasmtime_re.is_match(requirement))
    {
        // Having the wasmtime version hardcoded is not ideal, it's easy enough to overwrite
        metadata21
            .requires_dist
            .push("wasmtime>=5.0.0,<6.0.0".to_string());
    }

    Ok(metadata21)
}

/// Contains all the metadata required to build the crate
#[derive(Clone)]
pub struct BuildContext {
    /// The platform, i.e. os and pointer width
    pub target: Target,
    /// List of Cargo targets to compile
    pub cargo_targets: Vec<CompileTarget>,
    /// Whether this project is pure rust or rust mixed with python
    pub project_layout: ProjectLayout,
    /// The path to pyproject.toml. Required for the source distribution
    pub pyproject_toml_path: PathBuf,
    /// Parsed pyproject.toml if any
    pub pyproject_toml: Option<PyProjectToml>,
    /// Python Package Metadata 2.1
    pub metadata21: Metadata21,
    /// The name of the crate
    pub crate_name: String,
    /// The name of the module can be distinct from the package name, mostly
    /// because package names normally contain minuses while module names
    /// have underscores. The package name is part of metadata21
    pub module_name: String,
    /// The path to the Cargo.toml. Required for the cargo invocations
    pub manifest_path: PathBuf,
    /// Directory for all generated artifacts
    pub target_dir: PathBuf,
    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    pub out: PathBuf,
    /// Build artifacts in release mode, with optimizations
    pub release: bool,
    /// Strip the library for minimum file size
    pub strip: bool,
    /// Skip checking the linked libraries for manylinux/musllinux compliance
    pub skip_auditwheel: bool,
    /// When compiling for manylinux, use zig as linker to ensure glibc version compliance
    #[cfg(feature = "zig")]
    pub zig: bool,
    /// Whether to use the the manylinux/musllinux or use the native linux tag (off)
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
}

/// The wheel file location and its Python version tag (e.g. `py3`).
///
/// For bindings the version tag contains the Python interpreter version
/// they bind against (e.g. `cp37`).
pub type BuiltWheelMetadata = (PathBuf, String);

impl BuildContext {
    /// Checks which kind of bindings we have (pyo3/rust-cypthon or cffi or bin) and calls the
    /// correct builder.
    pub fn build_wheels(&self) -> Result<Vec<BuiltWheelMetadata>> {
        use itertools::Itertools;

        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the wheels")?;

        let wheels = match self.bridge() {
            BridgeModel::Bin(None) => self.build_bin_wheel(None)?,
            BridgeModel::Bin(Some(..)) => self.build_bin_wheels(&self.interpreter)?,
            BridgeModel::Bindings(..) => self.build_binding_wheels(&self.interpreter)?,
            BridgeModel::BindingsAbi3(major, minor) => {
                let abi3_interps: Vec<_> = self
                    .interpreter
                    .iter()
                    .filter(|interp| interp.has_stable_api())
                    .cloned()
                    .collect();
                let non_abi3_interps: Vec<_> = self
                    .interpreter
                    .iter()
                    .filter(|interp| !interp.has_stable_api())
                    .cloned()
                    .collect();
                let mut built_wheels = Vec::new();
                if !abi3_interps.is_empty() {
                    built_wheels.extend(self.build_binding_wheel_abi3(
                        &abi3_interps,
                        *major,
                        *minor,
                    )?);
                }
                if !non_abi3_interps.is_empty() {
                    let interp_names: HashSet<_> = non_abi3_interps
                        .iter()
                        .map(|interp| match interp.interpreter_kind {
                            InterpreterKind::CPython => interp.implmentation_name.to_string(),
                            InterpreterKind::PyPy => "PyPy".to_string(),
                        })
                        .collect();
                    eprintln!(
                        "⚠️ Warning: {} does not yet support abi3 so the build artifacts will be version-specific.",
                        interp_names.iter().join(", ")
                    );
                    built_wheels.extend(self.build_binding_wheels(&non_abi3_interps)?);
                }
                built_wheels
            }
            BridgeModel::Cffi => self.build_cffi_wheel()?,
            BridgeModel::UniFfi => self.build_uniffi_wheel()?,
        };

        Ok(wheels)
    }

    /// Bridge model
    pub fn bridge(&self) -> &BridgeModel {
        // FIXME: currently we only allow multiple bin targets so bridges are all the same
        &self.cargo_targets[0].1
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

    fn auditwheel(
        &self,
        artifact: &BuildArtifact,
        platform_tag: &[PlatformTag],
        python_interpreter: Option<&PythonInterpreter>,
    ) -> Result<(Policy, Vec<Library>)> {
        if self.skip_auditwheel {
            return Ok((Policy::default(), Vec::new()));
        }

        if let Some(python_interpreter) = python_interpreter {
            if platform_tag.is_empty()
                && self.target.is_linux()
                && !python_interpreter.support_portable_wheels()
            {
                println!(
                    "🐍 Skipping auditwheel because {python_interpreter} does not support manylinux/musllinux wheels"
                );
                return Ok((Policy::default(), Vec::new()));
            }
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
                allow_linking_libpython,
            );
        }

        let tag = others.get(0).or_else(|| musllinux.get(0)).copied();
        get_policy_and_libs(artifact, tag, &self.target, allow_linking_libpython)
    }

    /// Add library search paths in Cargo target directory rpath when building in editable mode
    fn add_rpath(&self, artifacts: &[&BuildArtifact]) -> Result<()> {
        if self.editable && self.target.is_linux() && !artifacts.is_empty() {
            for artifact in artifacts {
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

    fn add_external_libs(
        &self,
        writer: &mut WheelWriter,
        artifacts: &[&BuildArtifact],
        ext_libs: &[Vec<Library>],
    ) -> Result<()> {
        if self.editable {
            return self.add_rpath(artifacts);
        }
        if ext_libs.iter().all(|libs| libs.is_empty()) {
            return Ok(());
        }

        patchelf::verify_patchelf()?;

        // Put external libs to ${module_name}.libs directory
        // See https://github.com/pypa/auditwheel/issues/89
        let mut libs_dir = self
            .project_layout
            .python_module
            .as_ref()
            .and_then(|py| py.file_name().map(|s| s.to_os_string()))
            .unwrap_or_else(|| self.module_name.clone().into());
        libs_dir.push(".libs");
        let libs_dir = PathBuf::from(libs_dir);
        writer.add_directory(&libs_dir)?;

        let temp_dir = tempfile::tempdir()?;
        let mut soname_map = HashMap::new();
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
            writer.add_file_with_permissions(libs_dir.join(new_soname), path, 0o755)?;
        }

        println!(
            "🖨  Copied external shared libraries to package {} directory:",
            libs_dir.display()
        );
        for lib_path in libs_copied {
            println!("    {}", lib_path.display());
        }

        // Currently artifact .so file always resides at ${module_name}/${module_name}.so
        let artifact_dir = Path::new(&self.module_name);
        for artifact in artifacts {
            let mut new_rpaths = patchelf::get_rpath(&artifact.path)?;
            // TODO: clean existing rpath entries if it's not pointed to a location within the wheel
            // See https://github.com/pypa/auditwheel/blob/353c24250d66951d5ac7e60b97471a6da76c123f/src/auditwheel/repair.py#L160
            let new_rpath = Path::new("$ORIGIN").join(relpath(&libs_dir, artifact_dir));
            new_rpaths.push(new_rpath.to_str().unwrap().to_string());
            let new_rpath = new_rpaths.join(":");
            patchelf::set_rpath(&artifact.path, &new_rpath)?;
        }
        Ok(())
    }

    fn add_pth(&self, writer: &mut WheelWriter) -> Result<()> {
        if self.editable {
            writer.add_pth(&self.project_layout, &self.metadata21)?;
        }
        Ok(())
    }

    fn excludes(&self, format: Format) -> Result<Option<Override>> {
        if let Some(pyproject) = self.pyproject_toml.as_ref() {
            let pyproject_dir = self
                .pyproject_toml_path
                .normalize()
                .with_context(|| {
                    format!(
                        "failed to normalize path `{}`",
                        self.pyproject_toml_path.display()
                    )
                })?
                .into_path_buf();
            if let Some(glob_patterns) = &pyproject.exclude() {
                let mut excludes = OverrideBuilder::new(pyproject_dir.parent().unwrap());
                for glob in glob_patterns
                    .iter()
                    .filter_map(|glob_pattern| glob_pattern.targets(format))
                {
                    excludes.add(glob)?;
                }
                return Ok(Some(excludes.build()?));
            }
        }
        Ok(None)
    }

    fn write_binding_wheel_abi3(
        &self,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
        major: u8,
        min_minor: u8,
    ) -> Result<BuiltWheelMetadata> {
        let platform = self
            .target
            .get_platform_tag(platform_tags, self.universal2)?;
        let tag = format!("cp{major}{min_minor}-abi3-{platform}");

        let mut writer = WheelWriter::new(
            &tag,
            &self.out,
            &self.metadata21,
            &[tag.clone()],
            self.excludes(Format::Wheel)?,
        )?;
        self.add_external_libs(&mut writer, &[&artifact], &[ext_libs])?;

        write_bindings_module(
            &mut writer,
            &self.project_layout,
            &self.module_name,
            &artifact.path,
            None,
            &self.target,
            self.editable,
            self.pyproject_toml.as_ref(),
        )
        .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(&mut writer, self.project_layout.data.as_deref())?;
        let wheel_path = writer.finish()?;
        Ok((wheel_path, format!("cp{major}{min_minor}")))
    }

    /// For abi3 we only need to build a single wheel and we don't even need a python interpreter
    /// for it
    pub fn build_binding_wheel_abi3(
        &self,
        interpreters: &[PythonInterpreter],
        major: u8,
        min_minor: u8,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        // On windows, we have picked an interpreter to set the location of python.lib,
        // otherwise it's none
        let python_interpreter = interpreters.get(0);
        let artifact = self.compile_cdylib(
            python_interpreter,
            Some(&self.project_layout.extension_name),
        )?;
        let (policy, external_libs) =
            self.auditwheel(&artifact, &self.platform_tag, python_interpreter)?;
        let platform_tags = if self.platform_tag.is_empty() {
            vec![policy.platform_tag()]
        } else {
            self.platform_tag.clone()
        };
        let (wheel_path, tag) = self.write_binding_wheel_abi3(
            artifact,
            &platform_tags,
            external_libs,
            major,
            min_minor,
        )?;

        println!(
            "📦 Built wheel for abi3 Python ≥ {}.{} to {}",
            major,
            min_minor,
            wheel_path.display()
        );
        wheels.push((wheel_path, tag));

        Ok(wheels)
    }

    fn write_binding_wheel(
        &self,
        python_interpreter: &PythonInterpreter,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
    ) -> Result<BuiltWheelMetadata> {
        let tag = python_interpreter.get_tag(&self.target, platform_tags, self.universal2)?;

        let mut writer = WheelWriter::new(
            &tag,
            &self.out,
            &self.metadata21,
            &[tag.clone()],
            self.excludes(Format::Wheel)?,
        )?;
        self.add_external_libs(&mut writer, &[&artifact], &[ext_libs])?;

        write_bindings_module(
            &mut writer,
            &self.project_layout,
            &self.module_name,
            &artifact.path,
            Some(python_interpreter),
            &self.target,
            self.editable,
            self.pyproject_toml.as_ref(),
        )
        .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(&mut writer, self.project_layout.data.as_deref())?;
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
    pub fn build_binding_wheels(
        &self,
        interpreters: &[PythonInterpreter],
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        for python_interpreter in interpreters {
            let artifact = self.compile_cdylib(
                Some(python_interpreter),
                Some(&self.project_layout.extension_name),
            )?;
            let (policy, external_libs) =
                self.auditwheel(&artifact, &self.platform_tag, Some(python_interpreter))?;
            let platform_tags = if self.platform_tag.is_empty() {
                vec![policy.platform_tag()]
            } else {
                self.platform_tag.clone()
            };
            let (wheel_path, tag) = self.write_binding_wheel(
                python_interpreter,
                artifact,
                &platform_tags,
                external_libs,
            )?;
            println!(
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
    ) -> Result<BuildArtifact> {
        let artifacts = compile(self, python_interpreter, &self.cargo_targets)
            .context("Failed to build a native library through cargo")?;
        let error_msg = "Cargo didn't build a cdylib. Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?";
        let artifacts = artifacts.get(0).context(error_msg)?;

        let mut artifact = artifacts
            .get("cdylib")
            .cloned()
            .ok_or_else(|| anyhow!(error_msg,))?;

        if let Some(extension_name) = extension_name {
            // globin has an issue parsing MIPS64 ELF, see https://github.com/m4b/goblin/issues/274
            // But don't fail the build just because we can't emit a warning
            let _ = warn_missing_py_init(&artifact.path, extension_name);
        }

        if self.editable || self.skip_auditwheel {
            return Ok(artifact);
        }
        // auditwheel repair will edit the file, so we need to copy it to avoid errors in reruns
        let artifact_path = &artifact.path;
        let maturin_build = artifact_path.parent().unwrap().join("maturin");
        fs::create_dir_all(&maturin_build)?;
        let new_artifact_path = maturin_build.join(artifact_path.file_name().unwrap());
        fs::copy(artifact_path, &new_artifact_path)?;
        artifact.path = new_artifact_path;
        Ok(artifact)
    }

    fn write_cffi_wheel(
        &self,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
    ) -> Result<BuiltWheelMetadata> {
        let (tag, tags) = self
            .target
            .get_universal_tags(platform_tags, self.universal2)?;

        let mut writer = WheelWriter::new(
            &tag,
            &self.out,
            &self.metadata21,
            &tags,
            self.excludes(Format::Wheel)?,
        )?;
        self.add_external_libs(&mut writer, &[&artifact], &[ext_libs])?;

        write_cffi_module(
            &mut writer,
            &self.project_layout,
            self.manifest_path.parent().unwrap(),
            &self.target_dir,
            &self.module_name,
            &artifact.path,
            &self.interpreter[0].executable,
            self.editable,
            self.pyproject_toml.as_ref(),
        )?;

        self.add_pth(&mut writer)?;
        add_data(&mut writer, self.project_layout.data.as_deref())?;
        let wheel_path = writer.finish()?;
        Ok((wheel_path, "py3".to_string()))
    }

    /// Builds a wheel with cffi bindings
    pub fn build_cffi_wheel(&self) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        let artifact = self.compile_cdylib(None, None)?;
        let (policy, external_libs) = self.auditwheel(&artifact, &self.platform_tag, None)?;
        let platform_tags = if self.platform_tag.is_empty() {
            vec![policy.platform_tag()]
        } else {
            self.platform_tag.clone()
        };
        let (wheel_path, tag) = self.write_cffi_wheel(artifact, &platform_tags, external_libs)?;

        // Warn if cffi isn't specified in the requirements
        if !self
            .metadata21
            .requires_dist
            .iter()
            .any(|dep| dep.to_ascii_lowercase().starts_with("cffi"))
        {
            eprintln!(
                "⚠️  Warning: missing cffi package dependency, please add it to pyproject.toml. \
                e.g: `dependencies = [\"cffi\"]`. This will become an error."
            );
        }

        println!("📦 Built wheel to {}", wheel_path.display());
        wheels.push((wheel_path, tag));

        Ok(wheels)
    }

    fn write_uniffi_wheel(
        &self,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
    ) -> Result<BuiltWheelMetadata> {
        let (tag, tags) = self
            .target
            .get_universal_tags(platform_tags, self.universal2)?;

        let mut writer = WheelWriter::new(
            &tag,
            &self.out,
            &self.metadata21,
            &tags,
            self.excludes(Format::Wheel)?,
        )?;
        self.add_external_libs(&mut writer, &[&artifact], &[ext_libs])?;

        write_uniffi_module(
            &mut writer,
            &self.project_layout,
            self.manifest_path.parent().unwrap(),
            &self.target_dir,
            &self.module_name,
            &artifact.path,
            self.target.target_os(),
            self.editable,
            self.pyproject_toml.as_ref(),
        )?;

        self.add_pth(&mut writer)?;
        add_data(&mut writer, self.project_layout.data.as_deref())?;
        let wheel_path = writer.finish()?;
        Ok((wheel_path, "py3".to_string()))
    }

    /// Builds a wheel with uniffi bindings
    pub fn build_uniffi_wheel(&self) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        let artifact = self.compile_cdylib(None, None)?;
        let (policy, external_libs) = self.auditwheel(&artifact, &self.platform_tag, None)?;
        let platform_tags = if self.platform_tag.is_empty() {
            vec![policy.platform_tag()]
        } else {
            self.platform_tag.clone()
        };
        let (wheel_path, tag) = self.write_uniffi_wheel(artifact, &platform_tags, external_libs)?;

        println!("📦 Built wheel to {}", wheel_path.display());
        wheels.push((wheel_path, tag));

        Ok(wheels)
    }

    fn write_bin_wheel(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        artifacts: &[BuildArtifact],
        platform_tags: &[PlatformTag],
        ext_libs: &[Vec<Library>],
    ) -> Result<BuiltWheelMetadata> {
        let (tag, tags) = match (self.bridge(), python_interpreter) {
            (BridgeModel::Bin(None), _) => self
                .target
                .get_universal_tags(platform_tags, self.universal2)?,
            (BridgeModel::Bin(Some(..)), Some(python_interpreter)) => {
                let tag =
                    python_interpreter.get_tag(&self.target, platform_tags, self.universal2)?;
                (tag.clone(), vec![tag])
            }
            _ => unreachable!(),
        };

        if !self.metadata21.scripts.is_empty() {
            bail!("Defining scripts and working with a binary doesn't mix well");
        }

        let mut artifacts_and_files = Vec::new();
        for artifact in artifacts {
            // I wouldn't know of any case where this would be the wrong (and neither do
            // I know a better alternative)
            let bin_name = artifact
                .path
                .file_name()
                .context("Couldn't get the filename from the binary produced by cargo")?
                .to_str()
                .context("binary produced by cargo has non-utf8 filename")?
                .to_string();

            // From https://packaging.python.org/en/latest/specifications/entry-points/
            // > The name may contain any characters except =, but it cannot start or end with any
            // > whitespace character, or start with [. For new entry points, it is recommended to
            // > use only letters, numbers, underscores, dots and dashes (regex [\w.-]+).
            // All of these rules are already enforced by cargo:
            // https://github.com/rust-lang/cargo/blob/58a961314437258065e23cb6316dfc121d96fb71/src/cargo/util/restricted_names.rs#L39-L84
            // i.e. we don't need to do any bin name validation here anymore

            artifacts_and_files.push((artifact, bin_name))
        }

        let metadata21 = if self.target.is_wasi() {
            bin_wasi_helper(&artifacts_and_files, self.metadata21.clone())?
        } else {
            self.metadata21.clone()
        };

        let mut writer = WheelWriter::new(
            &tag,
            &self.out,
            &metadata21,
            &tags,
            self.excludes(Format::Wheel)?,
        )?;

        if self.project_layout.python_module.is_some() && self.target.is_wasi() {
            // TODO: Can we have python code and the wasm launchers coexisting
            // without clashes?
            bail!("Sorry, adding python code to a wasm binary is currently not supported")
        }
        if !self.editable {
            write_python_part(
                &mut writer,
                &self.project_layout,
                self.pyproject_toml.as_ref(),
            )
            .context("Failed to add the python module to the package")?;
        }

        let mut artifacts_ref = Vec::with_capacity(artifacts.len());
        for (artifact, bin_name) in &artifacts_and_files {
            artifacts_ref.push(*artifact);
            write_bin(&mut writer, &artifact.path, &self.metadata21, bin_name)?;
            if self.target.is_wasi() {
                write_wasm_launcher(&mut writer, &self.metadata21, bin_name)?;
            }
        }
        self.add_external_libs(&mut writer, &artifacts_ref, ext_libs)?;

        self.add_pth(&mut writer)?;
        add_data(&mut writer, self.project_layout.data.as_deref())?;
        let wheel_path = writer.finish()?;
        Ok((wheel_path, "py3".to_string()))
    }

    /// Builds a wheel that contains a binary
    ///
    /// Runs [auditwheel_rs()] if not deactivated
    pub fn build_bin_wheel(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        let artifacts = compile(self, python_interpreter, &self.cargo_targets)
            .context("Failed to build a native library through cargo")?;
        if artifacts.is_empty() {
            bail!("Cargo didn't build a binary")
        }

        let mut policies = Vec::with_capacity(artifacts.len());
        let mut ext_libs = Vec::new();
        let mut artifact_paths = Vec::with_capacity(artifacts.len());
        for artifact in artifacts {
            let artifact = artifact
                .get("bin")
                .cloned()
                .ok_or_else(|| anyhow!("Cargo didn't build a binary"))?;

            let (policy, external_libs) = self.auditwheel(&artifact, &self.platform_tag, None)?;
            policies.push(policy);
            ext_libs.push(external_libs);
            artifact_paths.push(artifact);
        }
        let policy = policies.iter().min_by_key(|p| p.priority).unwrap();
        let platform_tags = if self.platform_tag.is_empty() {
            vec![policy.platform_tag()]
        } else {
            self.platform_tag.clone()
        };

        let (wheel_path, tag) = self.write_bin_wheel(
            python_interpreter,
            &artifact_paths,
            &platform_tags,
            &ext_libs,
        )?;
        println!("📦 Built wheel to {}", wheel_path.display());
        wheels.push((wheel_path, tag));

        Ok(wheels)
    }

    /// Builds a wheel that contains a binary
    ///
    /// Runs [auditwheel_rs()] if not deactivated
    pub fn build_bin_wheels(
        &self,
        interpreters: &[PythonInterpreter],
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        for python_interpreter in interpreters {
            wheels.extend(self.build_bin_wheel(Some(python_interpreter))?);
        }
        Ok(wheels)
    }
}

/// Calculate the sha256 of a file
pub fn hash_file(path: impl AsRef<Path>) -> Result<String, io::Error> {
    let mut file = fs::File::open(path.as_ref())?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    let hex = format!("{:x}", hasher.finalize());
    Ok(hex)
}
