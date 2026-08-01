use anyhow::Result;
use fs_err as fs;
use maturin::{BuildOptions, BuildOrchestrator, CargoOptions, OutputOptions, PythonOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::instrument;

#[instrument(skip_all)]
pub fn generate_stubs(
    output: PathBuf,
    python_options: PythonOptions,
    cargo_options: CargoOptions,
) -> Result<()> {
    let temporary_wheels_dir = TempDir::new()?;
    let build_context = BuildOptions {
        python: python_options,
        cargo: cargo_options,
        generate_stubs: true,
        output: OutputOptions {
            out: Some(temporary_wheels_dir.path().into()),
            ..Default::default()
        },
        ..Default::default()
    }
    .into_build_context()
    .build()?;

    let orchestrator = BuildOrchestrator::new(&build_context);
    let mut stubs = orchestrator.generate_stubs()?;
    let project_layout = &build_context.project.project_layout;
    let extension_name = &project_layout.extension_name;
    // Lay the files out the way `--generate-stubs` lays them out inside a wheel, so that `--out`
    // can be copied over the python source tree as-is. Without the module directory,
    // `module-name = "a.b.c"` writes `c.pyi` to the root of `--out` instead of to `a/b/`, where
    // neither a type checker nor the package build would find it.
    let module_dir = output.join(project_layout.module_dir());

    if project_layout.python_module.is_some() {
        if stubs.len() == 1
            && let Some(stub) = stubs.remove(Path::new("__init__.pyi"))
        {
            // Special case, we generate just a `extension_name.pyi` file instead of a __init__.pyi
            // file: the extension is one module inside an existing package, not a package itself.
            write_stub(&module_dir.join(format!("{extension_name}.pyi")), &stub)?;
        } else {
            // We copy the files into a `extension_name` directory
            write_stub_dir(&module_dir.join(extension_name), &stubs)?;
        }
    } else {
        // Pure Rust project: maturin generates the package around the extension, so these stubs are
        // that package's own and keep their `__init__.pyi` name.
        write_stub_dir(&module_dir, &stubs)?;
    }
    Ok(())
}

fn write_stub(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

fn write_stub_dir(dir: &Path, stubs: &HashMap<PathBuf, String>) -> Result<()> {
    if dir.exists() {
        // We want to replace the stubs so we remove the directory
        fs::remove_dir_all(dir)?;
    }
    fs::create_dir_all(dir)?;
    for (path, content) in stubs {
        write_stub(&dir.join(path), content)?;
    }
    Ok(())
}
