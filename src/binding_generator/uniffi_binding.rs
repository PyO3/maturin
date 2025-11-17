use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use fs_err as fs;
use fs_err::File;
use normpath::PathExt as _;
use tracing::debug;

use crate::ModuleWriter;
use crate::PyProjectToml;
use crate::module_writer::write_python_part;
use crate::project_layout::ProjectLayout;
use crate::target::Os;

/// uniffi.toml
#[derive(Debug, serde::Deserialize)]
struct UniFfiToml {
    #[serde(default)]
    bindings: HashMap<String, UniFfiBindingsConfig>,
}

/// `bindings` section of uniffi.toml
#[derive(Debug, serde::Deserialize)]
struct UniFfiBindingsConfig {
    cdylib_name: Option<String>,
}

#[derive(Debug, Clone)]
struct UniFfiBindings {
    names: Vec<String>,
    cdylib: String,
    path: PathBuf,
}

fn uniffi_bindgen_command(crate_dir: &Path) -> Result<Command> {
    let manifest_path = crate_dir.join("Cargo.toml");
    let cargo_metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&manifest_path)
        // We don't need to resolve the dependency graph
        .no_deps()
        .verbose(true)
        .exec()?;
    let root_pkg = match cargo_metadata.root_package() {
        Some(pkg) => Some(pkg),
        None => cargo_metadata
            .packages
            .iter()
            .find(|p| p.manifest_path == manifest_path),
    };

    let has_uniffi_bindgen_target = root_pkg
        .map(|pkg| {
            pkg.targets
                .iter()
                .any(|target| target.name == "uniffi-bindgen" && target.is_bin())
        })
        .unwrap_or(false);
    let has_uniffi_bindgen_workspace_package = cargo_metadata.packages.iter().any(|pkg| {
        pkg.targets
            .iter()
            .any(|target| target.name == "uniffi-bindgen" && target.is_bin())
    });

    let command = if has_uniffi_bindgen_target {
        let mut command = Command::new("cargo");
        command
            .args(["run", "--bin", "uniffi-bindgen", "--manifest-path"])
            .arg(manifest_path)
            .current_dir(crate_dir)
            .env_remove("CARGO_BUILD_TARGET");
        command
    } else if has_uniffi_bindgen_workspace_package {
        let mut command = Command::new("cargo");
        command
            .args(["run", "--bin", "uniffi-bindgen"])
            .current_dir(cargo_metadata.workspace_root)
            .env_remove("CARGO_BUILD_TARGET");
        command
    } else {
        let mut command = Command::new("uniffi-bindgen");
        command.current_dir(crate_dir);
        command
    };
    Ok(command)
}

fn generate_uniffi_bindings(
    crate_dir: &Path,
    target_dir: &Path,
    module_name: &str,
    target_os: Os,
    artifact: &Path,
) -> Result<UniFfiBindings> {
    // `binding_dir` must use absolute path because we chdir to `crate_dir`
    // when running uniffi-bindgen
    let binding_dir = target_dir
        .normalize()?
        .join(env!("CARGO_PKG_NAME"))
        .join("uniffi")
        .join(module_name)
        .into_path_buf();
    fs::create_dir_all(&binding_dir)?;

    let mut cmd = uniffi_bindgen_command(crate_dir)?;
    cmd.args([
        "generate",
        "--no-format",
        "--language",
        "python",
        "--out-dir",
    ]);
    cmd.arg(&binding_dir);

    let config_file = crate_dir.join("uniffi.toml");
    let mut cdylib_name = None;
    if config_file.is_file() {
        let uniffi_toml: UniFfiToml = toml::from_str(&fs::read_to_string(&config_file)?)?;
        cdylib_name = uniffi_toml
            .bindings
            .get("python")
            .and_then(|py| py.cdylib_name.clone());

        // TODO: is this needed? `uniffi-bindgen` uses `uniffi.toml` by default,
        // `uniffi_bindgen_command` sets cwd to the crate (workspace) root, so maybe
        // we don't need to pass the config file explicitly?
        cmd.arg("--config");
        cmd.arg(config_file);
    }

    cmd.arg("--library");
    cmd.arg(artifact);

    debug!("Running {:?}", cmd);
    let mut child = cmd.spawn().context(
        "Failed to run uniffi-bindgen, did you install it? Try `pip install uniffi-bindgen`",
    )?;
    let exit_status = child.wait().context("Failed to run uniffi-bindgen")?;
    if !exit_status.success() {
        bail!("Command {:?} failed", cmd);
    }

    // Name of the cdylib is either from uniffi.toml or derived from the library
    let cdylib = match cdylib_name {
        // this logic should match with uniffi's expected names, e.g.
        // https://github.com/mozilla/uniffi-rs/blob/86a34083dd18bdd33f420c602b4fad624cc1e404/uniffi_bindgen/src/bindings/python/templates/NamespaceLibraryTemplate.py#L14-L37
        Some(cdylib_name) => match target_os {
            Os::Macos => format!("lib{cdylib_name}.dylib"),
            Os::Windows => format!("{cdylib_name}.dll"),
            _ => format!("lib{cdylib_name}.so"),
        },
        None => artifact.file_name().unwrap().to_str().unwrap().to_string(),
    };

    let py_bindings: Vec<_> = fs::read_dir(&binding_dir)?
        .flatten()
        .filter(|file| file.path().extension().and_then(std::ffi::OsStr::to_str) == Some("py"))
        .map(|file| {
            file.path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();

    Ok(UniFfiBindings {
        names: py_bindings,
        cdylib,
        path: binding_dir,
    })
}

/// Creates the uniffi module with the shared library
#[allow(clippy::too_many_arguments)]
pub fn write_uniffi_module(
    writer: &mut impl ModuleWriter,
    project_layout: &ProjectLayout,
    crate_dir: &Path,
    target_dir: &Path,
    module_name: &str,
    artifact: &Path,
    target_os: Os,
    editable: bool,
    pyproject_toml: Option<&PyProjectToml>,
) -> Result<()> {
    let UniFfiBindings {
        names: binding_names,
        cdylib,
        path: binding_dir,
    } = generate_uniffi_bindings(crate_dir, target_dir, module_name, target_os, artifact)?;

    let py_init = binding_names
        .iter()
        .map(|name| format!("from .{name} import *  # NOQA\n"))
        .collect::<Vec<String>>()
        .join("");

    if !editable {
        write_python_part(writer, project_layout, pyproject_toml)
            .context("Failed to add the python module to the package")?;
    }

    let module;
    if let Some(python_module) = &project_layout.python_module {
        if editable {
            let base_path = python_module.join(&project_layout.extension_name);
            fs::create_dir_all(&base_path)?;
            let target = base_path.join(&cdylib);
            fs::copy(artifact, &target).context(format!(
                "Failed to copy {} to {}",
                artifact.display(),
                target.display()
            ))?;

            File::create(base_path.join("__init__.py"))?.write_all(py_init.as_bytes())?;

            for binding_name in binding_names.iter() {
                let target: PathBuf = base_path.join(binding_name).with_extension("py");
                fs::copy(binding_dir.join(binding_name).with_extension("py"), &target)
                    .with_context(|| {
                        format!("Failed to copy {:?} to {:?}", binding_dir.display(), target)
                    })?;
            }
        }

        let relative = project_layout
            .rust_module
            .strip_prefix(python_module.parent().unwrap())
            .unwrap();
        module = relative.join(&project_layout.extension_name);
    } else {
        module = PathBuf::from(module_name);
        let type_stub = project_layout
            .rust_module
            .join(format!("{module_name}.pyi"));
        if type_stub.exists() {
            eprintln!("📖 Found type stub file at {module_name}.pyi");
            writer.add_file(module.join("__init__.pyi"), type_stub)?;
            writer.add_bytes(module.join("py.typed"), None, b"")?;
        }
    };

    if !editable || project_layout.python_module.is_none() {
        writer.add_bytes(module.join("__init__.py"), None, py_init.as_bytes())?;
        for binding in binding_names.iter() {
            writer.add_file(
                module.join(binding).with_extension("py"),
                binding_dir.join(binding).with_extension("py"),
            )?;
        }
        writer.add_file_with_permissions(module.join(cdylib), artifact, 0o755)?;
    }

    Ok(())
}
