use crate::build_options::{extract_cargo_metadata_args, CargoOptions};
use crate::{CargoToml, Metadata21, PyProjectToml};
use anyhow::{bail, format_err, Context, Result};
use cargo_metadata::{Metadata, MetadataCommand};
use fs_err as fs;
use std::env;
use std::io;
use std::path::{Path, PathBuf};

const PYPROJECT_TOML: &str = "pyproject.toml";

/// Whether this project is pure rust or rust mixed with python and whether it has wheel data
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectLayout {
    /// Contains the canonicalized (i.e. absolute) path to the python part of the project
    /// If none, we have a rust crate compiled into a shared library with only some glue python for cffi
    /// If some, we have a python package that is extended by a native rust module.
    pub python_module: Option<PathBuf>,
    /// Contains the canonicalized (i.e. absolute) path to the rust part of the project
    pub rust_module: PathBuf,
    /// Rust extension name
    pub extension_name: String,
    /// The location of the wheel data, if any
    pub data: Option<PathBuf>,
}

/// Project resolver
#[derive(Clone, Debug)]
pub struct ProjectResolver {
    /// Project layout
    pub project_layout: ProjectLayout,
    /// Cargo.toml path
    pub cargo_toml_path: PathBuf,
    /// Parsed Cargo.toml
    pub cargo_toml: CargoToml,
    /// pyproject.toml path
    pub pyproject_toml_path: PathBuf,
    /// Parsed pyproject.toml
    pub pyproject_toml: Option<PyProjectToml>,
    /// Rust module name
    pub module_name: String,
    /// Python Package Metadata 2.1
    pub metadata21: Metadata21,
    /// Cargo options
    pub cargo_options: CargoOptions,
    /// Cargo.toml as resolved by [cargo_metadata]
    pub cargo_metadata: Metadata,
    /// maturin options specified in pyproject.toml
    pub pyproject_toml_maturin_options: Vec<&'static str>,
}

impl ProjectResolver {
    /// Resolve project layout
    pub fn resolve(
        cargo_manifest_path: Option<PathBuf>,
        mut cargo_options: CargoOptions,
    ) -> Result<Self> {
        let (manifest_file, pyproject_file) =
            Self::resolve_manifest_paths(cargo_manifest_path, &cargo_options)?;
        if !manifest_file.is_file() {
            bail!(
                "{} is not the path to a Cargo.toml",
                manifest_file.display()
            );
        }
        let cargo_toml = CargoToml::from_path(&manifest_file)?;
        cargo_toml.warn_deprecated_python_metadata();

        let manifest_dir = manifest_file.parent().unwrap();
        let pyproject_toml: Option<PyProjectToml> = if pyproject_file.is_file() {
            let pyproject =
                PyProjectToml::new(&pyproject_file).context("pyproject.toml is invalid")?;
            pyproject.warn_missing_maturin_version();
            pyproject.warn_missing_build_backend();
            Some(pyproject)
        } else {
            None
        };
        let pyproject = pyproject_toml.as_ref();
        let tool_maturin = pyproject.and_then(|p| p.maturin());

        let pyproject_toml_maturin_options = if let Some(tool_maturin) = tool_maturin {
            cargo_options.merge_with_pyproject_toml(tool_maturin.clone())
        } else {
            Vec::new()
        };

        let cargo_metadata = Self::resolve_cargo_metadata(&manifest_file, &cargo_options)?;

        let mut metadata21 =
            Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir, &cargo_metadata)
                .context("Failed to parse Cargo.toml into python metadata")?;
        if let Some(pyproject) = pyproject {
            let pyproject_dir = pyproject_file.parent().unwrap();
            metadata21.merge_pyproject_toml(&pyproject_dir, pyproject)?;
        }
        let extra_metadata = cargo_toml.remaining_core_metadata();

        let crate_name = &cargo_toml.package.name;

        // If the package name contains minuses, you must declare a module with
        // underscores as lib name
        let module_name = cargo_toml
            .lib
            .as_ref()
            .and_then(|lib| lib.name.as_ref())
            .unwrap_or(crate_name)
            .to_owned();

        // Only use extension name from extra metadata if it contains dot
        let extension_name = extra_metadata
            .name
            .as_ref()
            .filter(|name| name.contains('.'))
            .unwrap_or(&module_name);

        let project_root = if pyproject_file.is_file() {
            pyproject_file.parent().unwrap_or(manifest_dir)
        } else {
            manifest_dir
        };
        let py_root = match pyproject.and_then(|x| x.python_source()) {
            Some(py_src) => py_src.to_path_buf(),
            None => match extra_metadata.python_source.as_ref() {
                Some(py_src) => manifest_dir.join(py_src),
                None => project_root.to_path_buf(),
            },
        };
        let data = match pyproject.and_then(|x| x.data()) {
            Some(data) => {
                if data.is_absolute() {
                    Some(data.to_path_buf())
                } else {
                    Some(project_root.join(data))
                }
            }
            None => extra_metadata.data.as_ref().map(|data| {
                let data = Path::new(data);
                if data.is_absolute() {
                    data.to_path_buf()
                } else {
                    manifest_dir.join(data)
                }
            }),
        };
        let project_layout = ProjectLayout::determine(project_root, extension_name, py_root, data)?;
        Ok(Self {
            project_layout,
            cargo_toml_path: manifest_file,
            cargo_toml,
            pyproject_toml_path: pyproject_file,
            pyproject_toml,
            module_name,
            metadata21,
            cargo_options,
            cargo_metadata,
            pyproject_toml_maturin_options,
        })
    }

    /// Get cargo manifest file path and pyproject.toml path
    fn resolve_manifest_paths(
        cargo_manifest_path: Option<PathBuf>,
        cargo_options: &CargoOptions,
    ) -> Result<(PathBuf, PathBuf)> {
        // use command line argument if specified
        if let Some(path) = cargo_manifest_path {
            let workspace_root = Self::resolve_cargo_metadata(&path, cargo_options)?.workspace_root;
            for parent in fs::canonicalize(&path)?.ancestors().skip(1) {
                if !parent.starts_with(&workspace_root) {
                    break;
                }
                let pyproject_file = parent.join(PYPROJECT_TOML);
                if pyproject_file.is_file() {
                    // Don't return canonicalized manifest path
                    // cargo doesn't handle them well.
                    // See https://github.com/rust-lang/cargo/issues/9770
                    return Ok((path, pyproject_file));
                }
            }
            return Ok((path.clone(), path.parent().unwrap().join(PYPROJECT_TOML)));
        }
        // check `manifest-path` option in pyproject.toml
        let current_dir = fs::canonicalize(
            env::current_dir().context("Failed to detect current directory ಠ_ಠ")?,
        )?;
        let pyproject_file = current_dir.join(PYPROJECT_TOML);
        if pyproject_file.is_file() {
            let pyproject =
                PyProjectToml::new(&pyproject_file).context("pyproject.toml is invalid")?;
            if let Some(path) = pyproject.manifest_path() {
                // pyproject.toml must be placed at top directory
                let manifest_dir =
                    fs::canonicalize(path.parent().context("missing parent directory")?)?;
                if !manifest_dir.starts_with(&current_dir) {
                    bail!("Cargo.toml can not be placed outside of the directory containing pyproject.toml");
                }
                return Ok((path.to_path_buf(), pyproject_file));
            }
        }
        // check Cargo.toml in current directory
        let path = PathBuf::from("Cargo.toml");
        if path.exists() {
            Ok((path, PathBuf::from(PYPROJECT_TOML)))
        } else {
            Err(format_err!(
                "Can't find {} (in {})",
                path.display(),
                current_dir.display()
            ))
        }
    }

    fn resolve_cargo_metadata(
        manifest_path: &Path,
        cargo_options: &CargoOptions,
    ) -> Result<Metadata> {
        let cargo_metadata_extra_args = extract_cargo_metadata_args(cargo_options)?;
        let result = MetadataCommand::new()
            .manifest_path(manifest_path)
            .other_options(cargo_metadata_extra_args)
            .exec();

        let cargo_metadata = match result {
            Ok(cargo_metadata) => cargo_metadata,
            Err(cargo_metadata::Error::Io(inner)) if inner.kind() == io::ErrorKind::NotFound => {
                // NotFound is the specific error when cargo is not in PATH
                return Err(inner)
                    .context("Cargo metadata failed. Do you have cargo in your PATH?");
            }
            Err(err) => {
                return Err(err)
                    .context("Cargo metadata failed. Does your crate compile with `cargo build`?");
            }
        };
        Ok(cargo_metadata)
    }
}

impl ProjectLayout {
    /// Checks whether a python module exists besides Cargo.toml with the right name
    fn determine(
        project_root: impl AsRef<Path>,
        module_name: &str,
        python_root: PathBuf,
        data: Option<PathBuf>,
    ) -> Result<ProjectLayout> {
        // A dot in the module name means the extension module goes into the module folder specified by the path
        let parts: Vec<&str> = module_name.split('.').collect();
        let project_root = project_root.as_ref();
        let (python_module, rust_module, extension_name) = if parts.len() > 1 {
            let mut rust_module = python_root.clone();
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

        let data = if let Some(data) = data {
            if !data.is_dir() {
                bail!("No such data directory {}", data.display());
            }
            Some(data)
        } else if project_root.join(format!("{}.data", module_name)).is_dir() {
            Some(project_root.join(format!("{}.data", module_name)))
        } else {
            None
        };

        if python_module.is_dir() {
            if !python_module.join("__init__.py").is_file()
                && !python_module.join("__init__.pyi").is_file()
            {
                bail!("Found a directory with the module name ({}) next to Cargo.toml, which indicates a mixed python/rust project, but the directory didn't contain an __init__.py file.", module_name)
            }

            println!("🍹 Building a mixed python/rust project");

            Ok(ProjectLayout {
                python_module: Some(python_module),
                rust_module,
                extension_name,
                data,
            })
        } else {
            Ok(ProjectLayout {
                python_module: None,
                rust_module: project_root.to_path_buf(),
                extension_name,
                data,
            })
        }
    }
}
