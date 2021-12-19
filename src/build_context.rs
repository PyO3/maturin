use crate::auditwheel::{
    auditwheel_rs, get_external_libs, hash_file, patchelf, PlatformTag, Policy,
};
use crate::compile::warn_missing_py_init;
use crate::module_writer::{
    write_bin, write_bindings_module, write_cffi_module, write_python_part, WheelWriter,
};
use crate::python_interpreter::InterpreterKind;
use crate::source_distribution::source_distribution;
use crate::{compile, Metadata21, ModuleWriter, PyProjectToml, PythonInterpreter, Target};
use anyhow::{anyhow, bail, Context, Result};
use cargo_metadata::Metadata;
use fs_err as fs;
use lddtree::Library;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The way the rust code is used in the wheel
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeModel {
    /// A native module with c bindings, i.e. `#[no_mangle] extern "C" <some item>`
    Cffi,
    /// A rust binary to be shipped a python package
    Bin,
    /// A native module with pyo3 or rust-cpython bindings. The String is the name of the bindings
    /// providing crate, e.g. pyo3.
    Bindings(String),
    /// `Bindings`, but specifically for pyo3 with feature flags that allow building a single wheel
    /// for all cpython versions (pypy still needs multiple versions).
    /// The numbers are the minimum major and minor version
    BindingsAbi3(u8, u8),
}

impl BridgeModel {
    /// Returns the name of the bindings crate
    pub fn unwrap_bindings(&self) -> &str {
        match self {
            BridgeModel::Bindings(value) => value,
            _ => panic!("Expected Bindings"),
        }
    }

    /// Test whether this is using a specific bindings crate
    pub fn is_bindings(&self, name: &str) -> bool {
        match self {
            BridgeModel::Bindings(value) => value == name,
            _ => false,
        }
    }
}

/// Whether this project is pure rust or rust mixed with python
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectLayout {
    /// A rust crate compiled into a shared library with only some glue python for cffi
    PureRust {
        /// Contains the canonicialized (i.e. absolute) path to the rust part of the project
        rust_module: PathBuf,
        /// rust extension name
        extension_name: String,
    },
    /// A python package that is extended by a native rust module.
    Mixed {
        /// Contains the canonicialized (i.e. absolute) path to the python part of the project
        python_module: PathBuf,
        /// Contains the canonicialized (i.e. absolute) path to the rust part of the project
        rust_module: PathBuf,
        /// rust extension name
        extension_name: String,
    },
}

impl ProjectLayout {
    /// Checks whether a python module exists besides Cargo.toml with the right name
    pub fn determine(
        project_root: impl AsRef<Path>,
        module_name: &str,
        py_src: Option<impl AsRef<Path>>,
    ) -> Result<ProjectLayout> {
        // A dot in the module name means the extension module goes into the module folder specified by the path
        let parts: Vec<&str> = module_name.split('.').collect();
        let project_root = project_root.as_ref();
        let python_root = py_src.map_or(Cow::Borrowed(project_root), |py_src| {
            Cow::Owned(project_root.join(py_src))
        });
        let (python_module, rust_module, extension_name) = if parts.len() > 1 {
            let mut rust_module = project_root.to_path_buf();
            rust_module.extend(&parts[0..parts.len() - 1]);
            (
                python_root.join(parts[0]),
                rust_module,
                parts[parts.len() - 1].to_string(),
            )
        } else {
            (
                python_root.join(module_name),
                python_root.join(module_name),
                module_name.to_string(),
            )
        };
        if python_module.is_dir() {
            if !python_module.join("__init__.py").is_file() {
                bail!("Found a directory with the module name ({}) next to Cargo.toml, which indicates a mixed python/rust project, but the directory didn't contain an __init__.py file.", module_name)
            }

            println!("🍹 Building a mixed python/rust project");

            Ok(ProjectLayout::Mixed {
                python_module,
                rust_module,
                extension_name,
            })
        } else {
            Ok(ProjectLayout::PureRust {
                rust_module: project_root.to_path_buf(),
                extension_name,
            })
        }
    }

    pub fn extension_name(&self) -> &str {
        match *self {
            ProjectLayout::PureRust {
                ref extension_name, ..
            } => extension_name,
            ProjectLayout::Mixed {
                ref extension_name, ..
            } => extension_name,
        }
    }
}

/// Contains all the metadata required to build the crate
#[derive(Clone)]
pub struct BuildContext {
    /// The platform, i.e. os and pointer width
    pub target: Target,
    /// Whether to use cffi or pyo3/rust-cpython
    pub bridge: BridgeModel,
    /// Whether this project is pure rust or rust mixed with python
    pub project_layout: ProjectLayout,
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
    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    pub out: PathBuf,
    /// Pass --release to cargo
    pub release: bool,
    /// Strip the library for minimum file size
    pub strip: bool,
    /// Whether to skip checking the linked libraries for manylinux/musllinux compliance
    pub skip_auditwheel: bool,
    /// Whether to use the the manylinux/musllinux or use the native linux tag (off)
    pub platform_tag: Option<PlatformTag>,
    /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] --`
    pub cargo_extra_args: Vec<String>,
    /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [arg1] [arg2]`
    pub rustc_extra_args: Vec<String>,
    /// The available python interpreter
    pub interpreter: Vec<PythonInterpreter>,
    /// Cargo.toml as resolved by [cargo_metadata]
    pub cargo_metadata: Metadata,
    /// Whether to use universal2 or use the native macOS tag (off)
    pub universal2: bool,
    /// Build editable wheels
    pub editable: bool,
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
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the wheels")?;

        let wheels = match &self.bridge {
            BridgeModel::Cffi => self.build_cffi_wheel()?,
            BridgeModel::Bin => self.build_bin_wheel()?,
            BridgeModel::Bindings(_) => self.build_binding_wheels(&self.interpreter)?,
            BridgeModel::BindingsAbi3(major, minor) => {
                let cpythons: Vec<_> = self
                    .interpreter
                    .iter()
                    .filter(|interp| interp.interpreter_kind == InterpreterKind::CPython)
                    .cloned()
                    .collect();
                let pypys: Vec<_> = self
                    .interpreter
                    .iter()
                    .filter(|interp| interp.interpreter_kind == InterpreterKind::PyPy)
                    .cloned()
                    .collect();
                let mut built_wheels = Vec::new();
                if !cpythons.is_empty() {
                    built_wheels.extend(self.build_binding_wheel_abi3(&cpythons, *major, *minor)?);
                }
                if !pypys.is_empty() {
                    println!(
                        "⚠️ Warning: PyPy does not yet support abi3 so the build artifacts will be version-specific. \
                        See https://foss.heptapod.net/pypy/pypy/-/issues/3397 for more information."
                    );
                    built_wheels.extend(self.build_binding_wheels(&pypys)?);
                }
                built_wheels
            }
        };

        Ok(wheels)
    }

    /// Builds a source distribution and returns the same metadata as [BuildContext::build_wheels]
    pub fn build_source_distribution(&self) -> Result<Option<BuiltWheelMetadata>> {
        fs::create_dir_all(&self.out)
            .context("Failed to create the target directory for the source distribution")?;

        let include_cargo_lock = self
            .cargo_extra_args
            .iter()
            .any(|arg| arg == "--locked" || arg == "--frozen");
        match PyProjectToml::new(self.manifest_path.parent().unwrap()) {
            Ok(pyproject) => {
                let sdist_path = source_distribution(
                    &self.out,
                    &self.metadata21,
                    &self.manifest_path,
                    &self.cargo_metadata,
                    pyproject.sdist_include(),
                    include_cargo_lock,
                )
                .context("Failed to build source distribution")?;
                Ok(Some((sdist_path, "source".to_string())))
            }
            Err(_) => Ok(None),
        }
    }

    fn auditwheel(
        &self,
        python_interpreter: Option<&PythonInterpreter>,
        artifact: &Path,
        platform_tag: Option<PlatformTag>,
    ) -> Result<(Policy, Vec<Library>)> {
        if self.skip_auditwheel {
            return Ok((Policy::default(), Vec::new()));
        }

        let target = python_interpreter
            .map(|x| &x.target)
            .unwrap_or(&self.target);

        let (policy, should_repair) =
            auditwheel_rs(artifact, target, platform_tag).with_context(|| {
                if let Some(platform_tag) = platform_tag {
                    format!("Error ensuring {} compliance", platform_tag)
                } else {
                    "Error checking for manylinux/musllinux compliance".to_string()
                }
            })?;
        let external_libs = if should_repair && !self.editable {
            get_external_libs(&artifact, &policy).with_context(|| {
                if let Some(platform_tag) = platform_tag {
                    format!("Error repairing wheel for {} compliance", platform_tag)
                } else {
                    "Error repairing wheel for manylinux/musllinux compliance".to_string()
                }
            })?
        } else {
            Vec::new()
        };
        Ok((policy, external_libs))
    }

    fn add_external_libs(
        &self,
        writer: &mut WheelWriter,
        artifact: &Path,
        ext_libs: &[Library],
    ) -> Result<()> {
        if ext_libs.is_empty() {
            return Ok(());
        }
        // Put external libs to ${module_name}.libs directory
        // See https://github.com/pypa/auditwheel/issues/89
        let libs_dir = PathBuf::from(format!("{}.libs", self.module_name));
        writer.add_directory(&libs_dir)?;

        let temp_dir = tempfile::tempdir()?;
        let mut soname_map = HashMap::new();
        for lib in ext_libs {
            let lib_path = lib.realpath.clone().with_context(|| {
                format!(
                    "Cannot repair wheel, because required library {} could not be located.",
                    lib.path.display()
                )
            })?;
            let short_hash = &hash_file(&lib_path)?[..8];
            let (file_stem, file_ext) = lib.name.split_once('.').unwrap();
            let new_soname = if !file_stem.ends_with(&format!("-{}", short_hash)) {
                format!("{}-{}.{}", file_stem, short_hash, file_ext)
            } else {
                format!("{}.{}", file_stem, file_ext)
            };
            let dest_path = temp_dir.path().join(&new_soname);
            fs::copy(&lib_path, &dest_path)?;
            patchelf::set_soname(&dest_path, &new_soname)?;
            if !lib.rpath.is_empty() || !lib.runpath.is_empty() {
                patchelf::set_rpath(&dest_path, &libs_dir)?;
            }
            soname_map.insert(
                lib.name.clone(),
                (new_soname.clone(), dest_path.clone(), lib.needed.clone()),
            );

            patchelf::replace_needed(artifact, &lib.name, &new_soname)?;
        }

        // we grafted in a bunch of libraries and modified their sonames, but
        // they may have internal dependencies (DT_NEEDED) on one another, so
        // we need to update those records so each now knows about the new
        // name of the other.
        for (new_soname, path, needed) in soname_map.values() {
            for n in needed {
                if soname_map.contains_key(n) {
                    patchelf::replace_needed(path, n, &soname_map[n].0)?;
                }
            }
            writer.add_file_with_permissions(libs_dir.join(new_soname), path, 0o755)?;
        }

        // Currently artifact .so file always resides at ${module_name}/${module_name}.so
        let artifact_dir = Path::new(&self.module_name);
        let old_rpaths = patchelf::get_rpath(artifact)?;
        // TODO: clean existing rpath entries if it's not pointed to a location within the wheel
        // See https://github.com/pypa/auditwheel/blob/353c24250d66951d5ac7e60b97471a6da76c123f/src/auditwheel/repair.py#L160
        let mut new_rpaths: Vec<&str> = old_rpaths.split(':').collect();
        let new_rpath = Path::new("$ORIGIN").join(relpath(&libs_dir, artifact_dir));
        new_rpaths.push(new_rpath.to_str().unwrap());
        let new_rpath = new_rpaths.join(":");
        patchelf::set_rpath(artifact, &new_rpath)?;
        Ok(())
    }

    fn add_pth(&self, writer: &mut WheelWriter) -> Result<()> {
        if self.editable {
            writer.add_pth(&self.project_layout, &self.metadata21)?;
        }
        Ok(())
    }

    fn write_binding_wheel_abi3(
        &self,
        artifact: &Path,
        platform_tag: PlatformTag,
        ext_libs: &[Library],
        major: u8,
        min_minor: u8,
    ) -> Result<BuiltWheelMetadata> {
        let platform = self
            .target
            .get_platform_tag(platform_tag, self.universal2)?;
        let tag = format!("cp{}{}-abi3-{}", major, min_minor, platform);

        let mut writer = WheelWriter::new(&tag, &self.out, &self.metadata21, &[tag.clone()])?;
        self.add_external_libs(&mut writer, artifact, ext_libs)?;

        write_bindings_module(
            &mut writer,
            &self.project_layout,
            &self.module_name,
            artifact,
            None,
            &self.target,
            self.editable,
        )
        .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;

        let wheel_path = writer.finish()?;
        Ok((wheel_path, format!("cp{}{}", major, min_minor)))
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
            Some(self.project_layout.extension_name()),
        )?;
        let (policy, external_libs) =
            self.auditwheel(python_interpreter, &artifact, self.platform_tag)?;
        let (wheel_path, tag) = self.write_binding_wheel_abi3(
            &artifact,
            policy.platform_tag(),
            &external_libs,
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
        artifact: &Path,
        platform_tag: PlatformTag,
        ext_libs: &[Library],
    ) -> Result<BuiltWheelMetadata> {
        let tag = python_interpreter.get_tag(platform_tag, self.universal2)?;

        let mut writer = WheelWriter::new(&tag, &self.out, &self.metadata21, &[tag.clone()])?;
        self.add_external_libs(&mut writer, artifact, ext_libs)?;

        write_bindings_module(
            &mut writer,
            &self.project_layout,
            &self.module_name,
            artifact,
            Some(python_interpreter),
            &self.target,
            self.editable,
        )
        .context("Failed to add the files to the wheel")?;

        self.add_pth(&mut writer)?;

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
                Some(self.project_layout.extension_name()),
            )?;
            let (policy, external_libs) =
                self.auditwheel(Some(python_interpreter), &artifact, self.platform_tag)?;
            let (wheel_path, tag) = self.write_binding_wheel(
                python_interpreter,
                &artifact,
                policy.platform_tag(),
                &external_libs,
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
    ) -> Result<PathBuf> {
        let artifacts = compile(self, python_interpreter, &self.bridge)
            .context("Failed to build a native library through cargo")?;

        let artifact = artifacts.get("cdylib").cloned().ok_or_else(|| {
            anyhow!(
                "Cargo didn't build a cdylib. Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?",
            )
        })?;

        if let Some(extension_name) = extension_name {
            warn_missing_py_init(&artifact, extension_name)
                .context("Failed to parse the native library")?;
        }

        if self.editable || self.skip_auditwheel {
            return Ok(artifact);
        }
        // auditwheel repair will edit the file, so we need to copy it to avoid errors in reruns
        let maturin_build = artifact.parent().unwrap().join("maturin");
        fs::create_dir_all(&maturin_build)?;
        let new_artifact = maturin_build.join(artifact.file_name().unwrap());
        fs::copy(&artifact, &new_artifact)?;
        Ok(new_artifact)
    }

    fn write_cffi_wheel(
        &self,
        artifact: &Path,
        platform_tag: PlatformTag,
        ext_libs: &[Library],
    ) -> Result<BuiltWheelMetadata> {
        let (tag, tags) = self
            .target
            .get_universal_tags(platform_tag, self.universal2)?;

        let mut writer = WheelWriter::new(&tag, &self.out, &self.metadata21, &tags)?;
        self.add_external_libs(&mut writer, artifact, ext_libs)?;

        write_cffi_module(
            &mut writer,
            &self.project_layout,
            self.manifest_path.parent().unwrap(),
            &self.module_name,
            artifact,
            &self.interpreter[0].executable,
            self.editable,
        )?;

        self.add_pth(&mut writer)?;

        let wheel_path = writer.finish()?;
        Ok((wheel_path, "py3".to_string()))
    }

    /// Builds a wheel with cffi bindings
    pub fn build_cffi_wheel(&self) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        let artifact = self.compile_cdylib(None, None)?;
        let (policy, external_libs) = self.auditwheel(None, &artifact, self.platform_tag)?;
        let (wheel_path, tag) =
            self.write_cffi_wheel(&artifact, policy.platform_tag(), &external_libs)?;

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

    fn write_bin_wheel(
        &self,
        artifact: &Path,
        platform_tag: PlatformTag,
        ext_libs: &[Library],
    ) -> Result<BuiltWheelMetadata> {
        let (tag, tags) = self
            .target
            .get_universal_tags(platform_tag, self.universal2)?;

        if !self.metadata21.scripts.is_empty() {
            bail!("Defining entrypoints and working with a binary doesn't mix well");
        }

        let mut writer = WheelWriter::new(&tag, &self.out, &self.metadata21, &tags)?;

        match self.project_layout {
            ProjectLayout::Mixed {
                ref python_module,
                ref extension_name,
                ..
            } => {
                if !self.editable {
                    write_python_part(&mut writer, python_module, extension_name)
                        .context("Failed to add the python module to the package")?;
                }
            }
            ProjectLayout::PureRust { .. } => {}
        }

        // I wouldn't know of any case where this would be the wrong (and neither do
        // I know a better alternative)
        let bin_name = artifact
            .file_name()
            .expect("Couldn't get the filename from the binary produced by cargo");
        self.add_external_libs(&mut writer, artifact, ext_libs)?;

        write_bin(&mut writer, artifact, &self.metadata21, bin_name)?;

        self.add_pth(&mut writer)?;

        let wheel_path = writer.finish()?;
        Ok((wheel_path, "py3".to_string()))
    }

    /// Builds a wheel that contains a binary
    ///
    /// Runs [auditwheel_rs()] if not deactivated
    pub fn build_bin_wheel(&self) -> Result<Vec<BuiltWheelMetadata>> {
        let mut wheels = Vec::new();
        let artifacts = compile(self, None, &self.bridge)
            .context("Failed to build a native library through cargo")?;

        let artifact = artifacts
            .get("bin")
            .cloned()
            .ok_or_else(|| anyhow!("Cargo didn't build a binary"))?;

        let (policy, external_libs) = self.auditwheel(None, &artifact, self.platform_tag)?;

        let (wheel_path, tag) =
            self.write_bin_wheel(&artifact, policy.platform_tag(), &external_libs)?;
        println!("📦 Built wheel to {}", wheel_path.display());
        wheels.push((wheel_path, tag));

        Ok(wheels)
    }
}

fn relpath(to: &Path, from: &Path) -> PathBuf {
    let mut suffix_pos = 0;
    for (f, t) in from.components().zip(to.components()) {
        if f == t {
            suffix_pos += 1;
        } else {
            break;
        }
    }
    let mut result = PathBuf::new();
    from.components()
        .skip(suffix_pos)
        .map(|_| result.push(".."))
        .last();
    to.components()
        .skip(suffix_pos)
        .map(|x| result.push(x.as_os_str()))
        .last();
    result
}

#[cfg(test)]
mod test {
    use super::relpath;
    use std::path::Path;

    #[test]
    fn test_relpath() {
        let cases = [
            ("", "", ""),
            ("/", "/usr", ".."),
            ("/", "/usr/lib", "../.."),
        ];
        for (from, to, expected) in cases {
            let from = Path::new(from);
            let to = Path::new(to);
            let result = relpath(from, to);
            assert_eq!(result, Path::new(expected));
        }
    }
}
