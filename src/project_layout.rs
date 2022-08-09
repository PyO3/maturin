use crate::{CargoToml, Metadata21, PyProjectToml};
use anyhow::{bail, format_err, Context, Result};
use std::env;
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
    /// Parsed pyproject.toml
    pub pyproject_toml: Option<PyProjectToml>,
    /// Rust module name
    pub module_name: String,
    /// Python Package Metadata 2.1
    pub metadata21: Metadata21,
}

impl ProjectResolver {
    /// Resolve project layout
    pub fn resolve(cargo_manifest_path: Option<PathBuf>) -> Result<Self> {
        let (manifest_file, pyproject_file) = Self::resolve_manifest_paths(cargo_manifest_path)?;
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

        let mut metadata21 = Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir)
            .context("Failed to parse Cargo.toml into python metadata")?;
        if let Some(pyproject) = pyproject {
            metadata21.merge_pyproject_toml(&manifest_dir, pyproject)?;
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
            pyproject_toml,
            module_name,
            metadata21,
        })
    }

    /// Get cargo manifest file path and pyproject.toml path
    fn resolve_manifest_paths(cargo_manifest_path: Option<PathBuf>) -> Result<(PathBuf, PathBuf)> {
        // use command line argument if specified
        if let Some(path) = cargo_manifest_path {
            return Ok((path.clone(), path.parent().unwrap().join(PYPROJECT_TOML)));
        }
        // check `manifest-path` option in pyproject.toml
        let current_dir = env::current_dir()
            .context("Failed to detect current directory ಠ_ಠ")?
            .canonicalize()?;
        let pyproject_file = current_dir.join(PYPROJECT_TOML);
        if pyproject_file.is_file() {
            let pyproject =
                PyProjectToml::new(&pyproject_file).context("pyproject.toml is invalid")?;
            if let Some(path) = pyproject.manifest_path() {
                // pyproject.toml must be placed at top directory
                let manifest_dir = path
                    .parent()
                    .context("missing parent directory")?
                    .canonicalize()?;
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
