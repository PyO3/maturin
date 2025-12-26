use crate::auditwheel::{AuditWheelMode, get_policy_and_libs, patchelf, relpath};
use crate::auditwheel::{PlatformTag, Policy};
use crate::binding_generator::{
    BinBindingGenerator, CffiBindingGenerator, Pyo3BindingGenerator, UniFfiBindingGenerator,
    generate_binding,
};
use crate::bridge::Abi3Version;
use crate::build_options::CargoOptions;
use crate::compile::{CompileTarget, warn_missing_py_init};
use crate::compression::CompressionOptions;
use crate::module_writer::{WheelWriter, add_data, write_pth};
use crate::project_layout::ProjectLayout;
use crate::source_distribution::source_distribution;
use crate::target::validate_wheel_filename_for_pypi;
use crate::target::{Arch, Os};
use crate::{
    BridgeModel, BuildArtifact, Metadata24, ModuleWriter, PyProjectToml, PythonInterpreter, Target,
    VirtualWriter, compile, pyproject_toml::Format,
};
use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::CrateType;
use cargo_metadata::Metadata;
use fs_err as fs;
use ignore::overrides::{Override, OverrideBuilder};
use lddtree::Library;
use normpath::PathExt;
use platform_info::*;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use tracing::instrument;
use zip::DateTime;

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
        use itertools::Itertools;

        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the wheels")?;

        let wheels = match self.bridge() {
            BridgeModel::Bin(None) => self.build_bin_wheel(None)?,
            BridgeModel::Bin(Some(..)) => self.build_bin_wheels(&self.interpreter)?,
            BridgeModel::PyO3(crate::PyO3 { abi3, .. }) => match abi3 {
                Some(Abi3Version::Version(major, minor)) => {
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
                        built_wheels.extend(self.build_pyo3_wheel_abi3(
                            &abi3_interps,
                            *major,
                            *minor,
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
                        built_wheels.extend(self.build_pyo3_wheels(&non_abi3_interps)?);
                    }
                    built_wheels
                }
                Some(Abi3Version::CurrentPython) => {
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
                        let interp = abi3_interps.first().unwrap();
                        built_wheels.extend(self.build_pyo3_wheel_abi3(
                            &abi3_interps,
                            interp.major as u8,
                            interp.minor as u8,
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
                        built_wheels.extend(self.build_pyo3_wheels(&non_abi3_interps)?);
                    }
                    built_wheels
                }
                None => self.build_pyo3_wheels(&self.interpreter)?,
            },
            BridgeModel::Cffi => self.build_cffi_wheel()?,
            BridgeModel::UniFfi => self.build_uniffi_wheel()?,
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
                self.get_universal_tags(&[PlatformTag::Linux])?.1
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

        if let Some(python_interpreter) = python_interpreter {
            if platform_tag.is_empty()
                && self.target.is_linux()
                && !python_interpreter.support_portable_wheels()
            {
                eprintln!(
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

        if matches!(self.auditwheel, AuditWheelMode::Check) {
            eprintln!(
                "🖨️ Your library is not manylinux/musllinux compliant because it requires copying the following libraries:"
            );
            for lib in ext_libs.iter().flatten() {
                if let Some(path) = lib.realpath.as_ref() {
                    eprintln!("    {} => {}", lib.name, path.display())
                } else {
                    eprintln!("    {} => not found", lib.name)
                };
            }
            bail!(
                "Can not repair the wheel because `--auditwheel=check` is specified, re-run with `--auditwheel=repair` to copy the libraries."
            );
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
            writer.add_file(libs_dir.join(new_soname), path, true)?;
        }

        eprintln!(
            "🖨  Copied external shared libraries to package {} directory:",
            libs_dir.display()
        );
        for lib_path in libs_copied {
            eprintln!("    {}", lib_path.display());
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
        if let Some(pyproject) = self.pyproject_toml.as_ref() {
            if let Some(glob_patterns) = &pyproject.exclude() {
                for glob in glob_patterns
                    .iter()
                    .filter_map(|glob_pattern| glob_pattern.targets(format))
                {
                    excludes.add(glob)?;
                }
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
        if let Ok(host_platform) = env::var("_PYTHON_HOST_PLATFORM") {
            let override_platform = host_platform.replace(['.', '-'], "_");
            eprintln!(
                "🚉 Overriding platform tag from _PYTHON_HOST_PLATFORM environment variable as {override_platform}."
            );
            return Ok(override_platform);
        }

        let target = &self.target;
        let tag = match (&target.target_os(), &target.target_arch()) {
            // Windows
            (Os::Windows, Arch::X86) => "win32".to_string(),
            (Os::Windows, Arch::X86_64) => "win_amd64".to_string(),
            (Os::Windows, Arch::Aarch64) => "win_arm64".to_string(),
            // Linux
            (Os::Linux, _) => {
                let arch = target.get_platform_arch()?;
                if target.target_triple().contains("android") {
                    let android_arch = match arch.as_str() {
                        "armv7l" => "armeabi_v7a",
                        "aarch64" => "arm64_v8a",
                        "i686" => "x86",
                        "x86_64" => "x86_64",
                        _ => bail!("Unsupported Android architecture: {}", arch),
                    };
                    let api_level = find_android_api_level(target.target_triple(), &self.manifest_path)?;
                    format!("android_{}_{}", api_level, android_arch)
                } else {
                    let mut platform_tags = platform_tags.to_vec();
                    platform_tags.sort();
                    let mut tags = vec![];
                    for platform_tag in platform_tags {
                        tags.push(format!("{platform_tag}_{arch}"));
                        for alias in platform_tag.aliases() {
                            tags.push(format!("{alias}_{arch}"));
                        }
                    }
                    tags.join(".")
                }
            }
            // macOS
            (Os::Macos, Arch::X86_64) | (Os::Macos, Arch::Aarch64) => {
                let ((x86_64_major, x86_64_minor), (arm64_major, arm64_minor)) = macosx_deployment_target(env::var("MACOSX_DEPLOYMENT_TARGET").ok().as_deref(), self.universal2)?;
                let x86_64_tag = if let Some(deployment_target) = self.pyproject_toml.as_ref().and_then(|x| x.target_config("x86_64-apple-darwin")).and_then(|config| config.macos_deployment_target.as_ref()) {
                    deployment_target.replace('.', "_")
                } else {
                    format!("{x86_64_major}_{x86_64_minor}")
                };
                let arm64_tag = if let Some(deployment_target) = self.pyproject_toml.as_ref().and_then(|x| x.target_config("aarch64-apple-darwin")).and_then(|config| config.macos_deployment_target.as_ref()) {
                    deployment_target.replace('.', "_")
                } else {
                    format!("{arm64_major}_{arm64_minor}")
                };
                if self.universal2 {
                    format!(
                        "macosx_{x86_64_tag}_x86_64.macosx_{arm64_tag}_arm64.macosx_{x86_64_tag}_universal2"
                    )
                } else if target.target_arch() == Arch::Aarch64 {
                    format!("macosx_{arm64_tag}_arm64")
                } else {
                    format!("macosx_{x86_64_tag}_x86_64")
                }
            }
            // iOS (simulator and device)
            (Os::Ios, Arch::X86_64) | (Os::Ios, Arch::Aarch64) => {
                let arch = if target.target_arch() == Arch::Aarch64 {
                    "arm64"
                } else {
                    "x86_64"
                };
                let abi = if target.target_arch() == Arch::X86_64 || self.target.target_triple().ends_with("-sim") {
                    "iphonesimulator"
                } else {
                    "iphoneos"
                };
                let (min_sdk_major, min_sdk_minor) = iphoneos_deployment_target(env::var("IPHONEOS_DEPLOYMENT_TARGET").ok().as_deref())?;
                format!("ios_{min_sdk_major}_{min_sdk_minor}_{arch}_{abi}")
            }
            // FreeBSD
            | (Os::FreeBsd, _) => {
                format!(
                    "{}_{}_{}",
                    target.target_os().to_string().to_ascii_lowercase(),
                    target.get_platform_release()?.to_ascii_lowercase(),
                    target.target_arch().machine(),
                )
            }
            // NetBSD
            | (Os::NetBsd, _)
            // OpenBSD
            | (Os::OpenBsd, _) => {
                let release = target.get_platform_release()?;
                format!(
                    "{}_{}_{}",
                    target.target_os().to_string().to_ascii_lowercase(),
                    release,
                    target.target_arch().machine(),
                )
            }
            // DragonFly
            (Os::Dragonfly, Arch::X86_64)
            // Haiku
            | (Os::Haiku, Arch::X86_64) => {
                let release = target.get_platform_release()?;
                format!(
                    "{}_{}_{}",
                    target.target_os().to_string().to_ascii_lowercase(),
                    release.to_ascii_lowercase(),
                    "x86_64"
                )
            }
            // Emscripten
            (Os::Emscripten, Arch::Wasm32) => {
                let release = emscripten_version()?.replace(['.', '-'], "_");
                format!("emscripten_{release}_wasm32")
            }
            (Os::Wasi, Arch::Wasm32) => {
                "any".to_string()
            }
            // Cygwin
            (Os::Cygwin, _) => {
                format!(
                    "{}_{}",
                    target.target_os().to_string().to_ascii_lowercase(),
                    target.get_platform_arch()?,
                )
            }
            // osname_release_machine fallback for any POSIX system
            (_, _) => {
                let info = PlatformInfo::new()
                    .map_err(|e| anyhow!("Failed to fetch platform information: {e}"))?;
                let mut release = info.release().to_string_lossy().replace(['.', '-'], "_");
                let mut machine = info.machine().to_string_lossy().replace([' ', '/'], "_");

                let mut os = target.target_os().to_string().to_ascii_lowercase();
                // See https://github.com/python/cpython/blob/46c8d915715aa2bd4d697482aa051fe974d440e1/Lib/sysconfig.py#L722-L730
                if target.target_os() == Os::Solaris || target.target_os() == Os::Illumos {
                    // Solaris / Illumos
                    if let Some((major, other)) = release.split_once('_') {
                        let major_ver: u64 = major.parse().context("illumos major version is not a number")?;
                        if major_ver >= 5 {
                            // SunOS 5 == Solaris 2
                            os = "solaris".to_string();
                            release = format!("{}_{}", major_ver - 3, other);
                            machine = format!("{machine}_64bit");
                        }
                    }
                }
                format!(
                    "{os}_{release}_{machine}"
                )
            }
        };
        Ok(tag)
    }

    /// Returns the tags for the WHEEL file for cffi wheels
    pub fn get_py3_tags(&self, platform_tags: &[PlatformTag]) -> Result<Vec<String>> {
        let tags = vec![format!(
            "py3-none-{}",
            self.get_platform_tag(platform_tags)?
        )];
        Ok(tags)
    }

    /// Returns the tags for the platform without python version
    pub fn get_universal_tags(
        &self,
        platform_tags: &[PlatformTag],
    ) -> Result<(String, Vec<String>)> {
        let tag = format!(
            "py3-none-{platform}",
            platform = self.get_platform_tag(platform_tags)?
        );
        let tags = self.get_py3_tags(platform_tags)?;
        Ok((tag, tags))
    }

    fn write_pyo3_wheel_abi3(
        &self,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
        major: u8,
        min_minor: u8,
    ) -> Result<BuiltWheelMetadata> {
        let platform = self.get_platform_tag(platform_tags)?;
        let tag = format!("cp{major}{min_minor}-abi3-{platform}");

        let file_options = self
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let writer = WheelWriter::new(&tag, &self.out, &self.metadata24, file_options)?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);
        self.add_external_libs(&mut writer, &[&artifact], &[ext_libs])?;

        let mut generator =
            Pyo3BindingGenerator::new(true, self.interpreter.first(), writer.temp_dir()?)
                .context("Failed to initialize PyO3 binding generator")?;
        generate_binding(&mut writer, &mut generator, self, &[&artifact])
            .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &self.metadata24,
            self.project_layout.data.as_deref(),
        )?;
        let wheel_path = writer.finish(
            &self.metadata24,
            &self.project_layout.project_root,
            std::slice::from_ref(&tag),
        )?;
        Ok((wheel_path, format!("cp{major}{min_minor}")))
    }

    /// For abi3 we only need to build a single wheel and we don't even need a python interpreter
    /// for it
    pub fn build_pyo3_wheel_abi3(
        &self,
        interpreters: &[PythonInterpreter],
        major: u8,
        min_minor: u8,
    ) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        // On windows, we have picked an interpreter to set the location of python.lib,
        // otherwise it's none
        let python_interpreter = interpreters.first();
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
        let (wheel_path, tag) =
            self.write_pyo3_wheel_abi3(artifact, &platform_tags, external_libs, major, min_minor)?;

        eprintln!(
            "📦 Built wheel for abi3 Python ≥ {}.{} to {}",
            major,
            min_minor,
            wheel_path.display()
        );
        wheels.push((wheel_path, tag));

        Ok(wheels)
    }

    fn write_pyo3_wheel(
        &self,
        python_interpreter: &PythonInterpreter,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
    ) -> Result<BuiltWheelMetadata> {
        let tag = python_interpreter.get_tag(self, platform_tags)?;

        let file_options = self
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let writer = WheelWriter::new(&tag, &self.out, &self.metadata24, file_options)?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);
        self.add_external_libs(&mut writer, &[&artifact], &[ext_libs])?;

        let mut generator =
            Pyo3BindingGenerator::new(false, Some(python_interpreter), writer.temp_dir()?)
                .context("Failed to initialize PyO3 binding generator")?;
        generate_binding(&mut writer, &mut generator, self, &[&artifact])
            .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &self.metadata24,
            self.project_layout.data.as_deref(),
        )?;
        let wheel_path = writer.finish(
            &self.metadata24,
            &self.project_layout.project_root,
            std::slice::from_ref(&tag),
        )?;
        Ok((
            wheel_path,
            format!("cp{}{}", python_interpreter.major, python_interpreter.minor),
        ))
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
            let (wheel_path, tag) =
                self.write_pyo3_wheel(python_interpreter, artifact, &platform_tags, external_libs)?;
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
    ) -> Result<BuildArtifact> {
        let artifacts = compile(self, python_interpreter, &self.compile_targets)
            .context("Failed to build a native library through cargo")?;
        let error_msg = "Cargo didn't build a cdylib. Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?";
        let artifacts = artifacts.first().context(error_msg)?;

        let mut artifact = artifacts
            .get(&CrateType::CDyLib)
            .cloned()
            .ok_or_else(|| anyhow!(error_msg,))?;

        if let Some(extension_name) = extension_name {
            // globin has an issue parsing MIPS64 ELF, see https://github.com/m4b/goblin/issues/274
            // But don't fail the build just because we can't emit a warning
            let _ = warn_missing_py_init(&artifact.path, extension_name);
        }

        if self.editable || matches!(self.auditwheel, AuditWheelMode::Skip) {
            return Ok(artifact);
        }
        // auditwheel repair will edit the file, so we need to copy it to avoid errors in reruns
        let maturin_build = self.target_dir.join(env!("CARGO_PKG_NAME"));
        fs::create_dir_all(&maturin_build)?;
        let artifact_path = &artifact.path;
        let new_artifact_path = maturin_build.join(artifact_path.file_name().unwrap());
        fs::copy(artifact_path, &new_artifact_path)?;
        artifact.path = new_artifact_path.normalize()?.into_path_buf();
        Ok(artifact)
    }

    fn write_cffi_wheel(
        &self,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
    ) -> Result<BuiltWheelMetadata> {
        let (tag, tags) = self.get_universal_tags(platform_tags)?;

        let file_options = self
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let writer = WheelWriter::new(&tag, &self.out, &self.metadata24, file_options)?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);
        self.add_external_libs(&mut writer, &[&artifact], &[ext_libs])?;

        let interpreter = self.interpreter.first().ok_or_else(|| {
            anyhow!("A python interpreter is required for cffi builds but one was not provided")
        })?;
        let mut generator = CffiBindingGenerator::new(interpreter, writer.temp_dir()?)
            .context("Failed to initialize Cffi binding generator")?;
        generate_binding(&mut writer, &mut generator, self, &[&artifact])?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &self.metadata24,
            self.project_layout.data.as_deref(),
        )?;
        let wheel_path =
            writer.finish(&self.metadata24, &self.project_layout.project_root, &tags)?;
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
        wheels.push((wheel_path, tag));

        Ok(wheels)
    }

    fn write_uniffi_wheel(
        &self,
        artifact: BuildArtifact,
        platform_tags: &[PlatformTag],
        ext_libs: Vec<Library>,
    ) -> Result<BuiltWheelMetadata> {
        let (tag, tags) = self.get_universal_tags(platform_tags)?;

        let file_options = self
            .compression
            .get_file_options()
            .last_modified_time(zip_mtime());
        let writer = WheelWriter::new(&tag, &self.out, &self.metadata24, file_options)?;
        let mut writer = VirtualWriter::new(writer, self.excludes(Format::Wheel)?);
        self.add_external_libs(&mut writer, &[&artifact], &[ext_libs])?;

        let mut generator = UniFfiBindingGenerator::default();
        generate_binding(&mut writer, &mut generator, self, &[&artifact])?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &self.metadata24,
            self.project_layout.data.as_deref(),
        )?;
        let wheel_path =
            writer.finish(&self.metadata24, &self.project_layout.project_root, &tags)?;
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

        eprintln!("📦 Built wheel to {}", wheel_path.display());
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
        if !self.metadata24.scripts.is_empty() {
            bail!("Defining scripts and working with a binary doesn't mix well");
        }

        if self.target.is_wasi() {
            eprintln!("⚠️  Warning: wasi support is experimental");
            // escaped can contain [\w\d.], but i don't know how we'd handle dots correctly here
            if self.metadata24.get_distribution_escaped().contains('.') {
                bail!(
                    "Can't build wasm wheel if there is a dot in the name ('{}')",
                    self.metadata24.get_distribution_escaped()
                )
            }

            if !self.metadata24.entry_points.is_empty() {
                bail!("You can't define entrypoints yourself for a binary project");
            }

            if self.project_layout.python_module.is_some() {
                // TODO: Can we have python code and the wasm launchers coexisting
                // without clashes?
                bail!("Sorry, adding python code to a wasm binary is currently not supported")
            }
        }

        let (tag, tags) = match (self.bridge(), python_interpreter) {
            (BridgeModel::Bin(None), _) => self.get_universal_tags(platform_tags)?,
            (BridgeModel::Bin(Some(..)), Some(python_interpreter)) => {
                let tag = python_interpreter.get_tag(self, platform_tags)?;
                (tag.clone(), vec![tag])
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
        generate_binding(&mut writer, &mut generator, self, artifacts)
            .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;
        add_data(
            &mut writer,
            &metadata24,
            self.project_layout.data.as_deref(),
        )?;
        let wheel_path = writer.finish(&metadata24, &self.project_layout.project_root, &tags)?;
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
        let artifacts = compile(self, python_interpreter, &self.compile_targets)
            .context("Failed to build a native library through cargo")?;
        if artifacts.is_empty() {
            bail!("Cargo didn't build a binary")
        }

        let mut policies = Vec::with_capacity(artifacts.len());
        let mut ext_libs = Vec::new();
        let mut artifact_paths = Vec::with_capacity(artifacts.len());
        for artifact in artifacts {
            let artifact = artifact
                .get(&CrateType::Bin)
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
        eprintln!("📦 Built wheel to {}", wheel_path.display());
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

/// Get the default macOS deployment target version
fn macosx_deployment_target(
    deploy_target: Option<&str>,
    universal2: bool,
) -> Result<((u16, u16), (u16, u16))> {
    let x86_64_default_rustc = rustc_macosx_target_version("x86_64-apple-darwin");
    let x86_64_default = if universal2 && x86_64_default_rustc.1 < 9 {
        (10, 9)
    } else {
        x86_64_default_rustc
    };
    let arm64_default = rustc_macosx_target_version("aarch64-apple-darwin");
    let mut x86_64_ver = x86_64_default;
    let mut arm64_ver = arm64_default;
    if let Some(deploy_target) = deploy_target {
        let err_ctx = "MACOSX_DEPLOYMENT_TARGET is invalid";
        let mut parts = deploy_target.split('.');
        let major = parts.next().context(err_ctx)?;
        let major: u16 = major.parse().context(err_ctx)?;
        let minor = parts.next().context(err_ctx)?;
        let minor: u16 = minor.parse().context(err_ctx)?;
        if (major, minor) > x86_64_default {
            x86_64_ver = (major, minor);
        }
        if (major, minor) > arm64_default {
            arm64_ver = (major, minor);
        }
    }
    Ok((
        python_macosx_target_version(x86_64_ver),
        python_macosx_target_version(arm64_ver),
    ))
}

/// Get the iOS deployment target version
fn iphoneos_deployment_target(deploy_target: Option<&str>) -> Result<(u16, u16)> {
    let (major, minor) = if let Some(deploy_target) = deploy_target {
        let err_ctx = "IPHONEOS_DEPLOYMENT_TARGET is invalid";
        let mut parts = deploy_target.split('.');
        let major = parts.next().context(err_ctx)?;
        let major: u16 = major.parse().context(err_ctx)?;
        let minor = parts.next().context(err_ctx)?;
        let minor: u16 = minor.parse().context(err_ctx)?;
        (major, minor)
    } else {
        (13, 0)
    };

    Ok((major, minor))
}

#[inline]
fn python_macosx_target_version(version: (u16, u16)) -> (u16, u16) {
    let (major, minor) = version;
    if major >= 11 {
        // pip only supports (major, 0) for macOS 11+
        (major, 0)
    } else {
        (major, minor)
    }
}

pub(crate) fn rustc_macosx_target_version(target: &str) -> (u16, u16) {
    use std::process::{Command, Stdio};
    use target_lexicon::OperatingSystem;

    // On rustc 1.71.0+ we can use `rustc --print deployment-target`
    if let Ok(output) = Command::new("rustc")
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .env_remove("MACOSX_DEPLOYMENT_TARGET")
        .args(["--target", target])
        .args(["--print", "deployment-target"])
        .output()
    {
        if output.status.success() {
            let target_version = std::str::from_utf8(&output.stdout)
                .unwrap()
                .split('=')
                .next_back()
                .and_then(|v| v.trim().split_once('.'));
            if let Some((major, minor)) = target_version {
                let major: u16 = major.parse().unwrap();
                let minor: u16 = minor.parse().unwrap();
                return (major, minor);
            }
        }
    }

    let fallback_version = if target == "aarch64-apple-darwin" {
        (11, 0)
    } else {
        (10, 7)
    };

    let rustc_target_version = || -> Result<(u16, u16)> {
        let cmd = Command::new("rustc")
            .arg("-Z")
            .arg("unstable-options")
            .arg("--print")
            .arg("target-spec-json")
            .arg("--target")
            .arg(target)
            .env("RUSTC_BOOTSTRAP", "1")
            .env_remove("MACOSX_DEPLOYMENT_TARGET")
            .output()
            .context("Failed to run rustc to get the target spec")?;
        let stdout = String::from_utf8(cmd.stdout).context("rustc output is not valid utf-8")?;
        let spec: serde_json::Value =
            serde_json::from_str(&stdout).context("rustc output is not valid json")?;
        let llvm_target = spec
            .as_object()
            .context("rustc output is not a json object")?
            .get("llvm-target")
            .context("rustc output does not contain llvm-target")?
            .as_str()
            .context("llvm-target is not a string")?;
        let triple = llvm_target.parse::<target_lexicon::Triple>();
        let (major, minor) = match triple.map(|t| t.operating_system) {
            Ok(
                OperatingSystem::MacOSX(Some(deployment_target))
                | OperatingSystem::Darwin(Some(deployment_target)),
            ) => (deployment_target.major, u16::from(deployment_target.minor)),
            _ => fallback_version,
        };
        Ok((major, minor))
    };
    rustc_target_version().unwrap_or(fallback_version)
}

/// Emscripten version
fn emscripten_version() -> Result<String> {
    let os_version = env::var("MATURIN_EMSCRIPTEN_VERSION");
    let release = match os_version {
        Ok(os_ver) => os_ver,
        Err(_) => emcc_version()?,
    };
    Ok(release)
}

fn emcc_version() -> Result<String> {
    use std::process::Command;

    let emcc = Command::new(if cfg!(windows) { "emcc.bat" } else { "emcc" })
        .arg("-dumpversion")
        .output()
        .context("Failed to run emcc to get the version")?;
    let ver = String::from_utf8(emcc.stdout)?;
    let mut trimmed = ver.trim();
    trimmed = trimmed.strip_suffix("-git").unwrap_or(trimmed);
    Ok(trimmed.into())
}

fn find_android_api_level(target_triple: &str, manifest_path: &Path) -> Result<String> {
    if let Ok(val) = env::var("ANDROID_API_LEVEL") {
        return Ok(val);
    }

    let mut clues = Vec::new();

    // 1. Linker from cargo-config2
    if let Some(manifest_dir) = manifest_path.parent() {
        if let Ok(config) = cargo_config2::Config::load_with_cwd(manifest_dir) {
            if let Ok(Some(linker)) = config.linker(target_triple) {
                clues.push(linker.to_string_lossy().into_owned());
            }
        }
    }

    // 2. CC env vars
    if let Ok(cc) = env::var(format!("CC_{}", target_triple.replace('-', "_"))) {
        clues.push(cc);
    }
    if let Ok(cc) = env::var("CC") {
        clues.push(cc);
    }

    // Search for android(\d+) in clues
    let re = Regex::new(r"android(\d+)")?;
    for clue in clues {
        if let Some(caps) = re.captures(&clue) {
            return Ok(caps[1].to_string());
        }
    }

    bail!(
        "Failed to determine Android API level. Please set the ANDROID_API_LEVEL environment variable."
    );
}

/// Returns a DateTime representing the value SOURCE_DATE_EPOCH environment variable
/// Note that the earliest timestamp a zip file can represent is 1980-01-01
fn zip_mtime() -> DateTime {
    let res = env::var("SOURCE_DATE_EPOCH")
        .context("") // Only using context() to unify the error types
        .and_then(|epoch| {
            let epoch: i64 = epoch.parse()?;
            let dt = time::OffsetDateTime::from_unix_timestamp(epoch)?;
            let dt = DateTime::try_from(dt)?;
            Ok(dt)
        });

    res.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{iphoneos_deployment_target, macosx_deployment_target};
    use pretty_assertions::assert_eq;

    #[test]
    fn test_macosx_deployment_target() {
        let rustc_ver = rustc_version::version().unwrap();
        let rustc_ver = (rustc_ver.major, rustc_ver.minor);
        let x86_64_minor = if rustc_ver >= (1, 74) { 12 } else { 7 };
        let universal2_minor = if rustc_ver >= (1, 74) { 12 } else { 9 };
        assert_eq!(
            macosx_deployment_target(None, false).unwrap(),
            ((10, x86_64_minor), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(None, true).unwrap(),
            ((10, universal2_minor), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.6"), false).unwrap(),
            ((10, x86_64_minor), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.6"), true).unwrap(),
            ((10, universal2_minor), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("10.9"), false).unwrap(),
            ((10, universal2_minor), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("11.0.0"), false).unwrap(),
            ((11, 0), (11, 0))
        );
        assert_eq!(
            macosx_deployment_target(Some("11.1"), false).unwrap(),
            ((11, 0), (11, 0))
        );
    }

    #[test]
    fn test_iphoneos_deployment_target() {
        // Use default when no value is provided
        assert_eq!(iphoneos_deployment_target(None).unwrap(), (13, 0));

        // Valid version strings
        assert_eq!(iphoneos_deployment_target(Some("13.0")).unwrap(), (13, 0));
        assert_eq!(iphoneos_deployment_target(Some("14.5")).unwrap(), (14, 5));
        assert_eq!(iphoneos_deployment_target(Some("15.0")).unwrap(), (15, 0));
        // Extra parts are ignored
        assert_eq!(iphoneos_deployment_target(Some("14.5.1")).unwrap(), (14, 5));

        // Invalid formats
        assert!(iphoneos_deployment_target(Some("invalid")).is_err());
        assert!(iphoneos_deployment_target(Some("13")).is_err());
        assert!(iphoneos_deployment_target(Some("13.")).is_err());
        assert!(iphoneos_deployment_target(Some(".0")).is_err());
        assert!(iphoneos_deployment_target(Some("abc.def")).is_err());
        assert!(iphoneos_deployment_target(Some("13.abc")).is_err());
        assert!(iphoneos_deployment_target(Some("")).is_err());
    }
}
