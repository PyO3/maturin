use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use fs_err as fs;
use normpath::PathExt as _;
use tracing::debug;

use crate::BuildArtifact;
use crate::BuildContext;
use crate::archive_source::ArchiveSource;
use crate::archive_source::FileSourceData;
use crate::archive_source::GeneratedSourceData;
use crate::target::Os;

use super::BindingGenerator;
use super::GeneratorOutput;

/// A generator for producing UniFFI bindings.
#[derive(Default)]
pub struct UniFfiBindingGenerator {}

impl BindingGenerator for UniFfiBindingGenerator {
    fn generate_bindings(
        &mut self,
        context: &BuildContext,
        artifact: &BuildArtifact,
        module: &Path,
    ) -> Result<GeneratorOutput> {
        let base_path = if context.project_layout.python_module.is_some() {
            module.join(&context.project_layout.extension_name)
        } else {
            module.to_path_buf()
        };

        let UniFfiBindings {
            names: binding_names,
            cdylib,
            path: binding_dir,
        } = generate_uniffi_bindings(
            context.manifest_path.parent().unwrap(),
            &context.target_dir,
            &context.module_name,
            context.target.target_os(),
            &artifact.path,
        )?;
        let artifact_target = base_path.join(cdylib);
        let mut additional_files = HashMap::new();

        let py_init = binding_names
            .iter()
            .map(|name| format!("from .{name} import *  # NOQA\n"))
            .collect::<Vec<String>>()
            .join("");
        additional_files.insert(
            base_path.join("__init__.py"),
            ArchiveSource::Generated(GeneratedSourceData {
                data: py_init.into(),
                path: None,
                executable: false,
            }),
        );

        for binding in binding_names {
            let filename = format!("{binding}.py");
            let source = FileSourceData {
                path: binding_dir.join(&filename),
                executable: false,
            };
            additional_files.insert(base_path.join(filename), ArchiveSource::File(source));
        }

        Ok(GeneratorOutput {
            artifact_target,
            artifact_source_override: None,
            additional_files: Some(additional_files),
        })
    }
}

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
            .find(|&p| p.manifest_path == manifest_path),
    };

    let uniffi_bindgen_target = root_pkg.and_then(|pkg| {
        pkg.targets
            .iter()
            .find(|&target| target.name == "uniffi-bindgen" && target.is_bin())
    });
    let uniffi_bindgen_workspace_target = cargo_metadata
        .packages
        .iter()
        .flat_map(|pkg| pkg.targets.iter())
        .find(|&target| target.name == "uniffi-bindgen" && target.is_bin());

    let command = if let Some(target) = uniffi_bindgen_target {
        let mut command = Command::new("cargo");
        command
            .args(["run", "--bin", "uniffi-bindgen", "--manifest-path"])
            .arg(manifest_path)
            .current_dir(crate_dir)
            .env_remove("CARGO_BUILD_TARGET");
        if !target.required_features.is_empty() {
            let features = target.required_features.join(",");
            command.arg("--features").arg(features);
        }
        command
    } else if let Some(target) = uniffi_bindgen_workspace_target {
        let mut command = Command::new("cargo");
        command
            .args(["run", "--bin", "uniffi-bindgen"])
            .current_dir(cargo_metadata.workspace_root)
            .env_remove("CARGO_BUILD_TARGET");
        if !target.required_features.is_empty() {
            let features = target.required_features.join(",");
            command.arg("--features").arg(features);
        }
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
