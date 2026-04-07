use anyhow::Result;
use fs_err as fs;
use maturin::{BuildOptions, BuildOrchestrator, CargoOptions, OutputOptions, PythonOptions};
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
    let stubs = orchestrator.generate_stubs()?;
    let extension_name = &build_context.project.project_layout.extension_name;
    if stubs.len() == 1
        && let Some(stub) = stubs.get(Path::new("__init__.pyi"))
    {
        // Special case, we generate just a `extension_name.pyi` file instead of a __init__.pyi file
        let output_path = output.join(format!("{}.pyi", extension_name));
        if let Some(output_parent) = output_path.parent() {
            fs::create_dir_all(output_parent)?;
        }
        fs::write(&output_path, stub)?;
    } else {
        // We copy the file into a `extension_name` directory
        let output_dir = output.join(extension_name);
        if output_dir.exists() {
            // We want to replace the stubs so we remove the directory
            fs::remove_dir_all(&output_dir)?;
        }
        fs::create_dir_all(&output_dir)?;
        for (path, content) in &stubs {
            let output_path = output_dir.join(path);
            if let Some(output_parent) = output_path.parent() {
                fs::create_dir_all(output_parent)?;
            }
            fs::write(&output_path, content)?;
        }
    }
    Ok(())
}
