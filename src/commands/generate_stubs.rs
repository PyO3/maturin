use anyhow::{Context, Result, ensure};
use fs_err as fs;
use maturin::{BuildOptions, BuildOrchestrator, CargoOptions, OutputOptions, PythonOptions};
use std::io;
use std::path::PathBuf;
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
    let wheels = orchestrator.build_wheels()?;
    let mut found_stubs = false;
    for wheel in wheels {
        let mut archive = zip::ZipArchive::new(fs::File::open(&wheel.0)?)
            .with_context(|| format!("Failed to open wheel {}", wheel.0.display()))?;
        for idx in 0..archive.len() {
            let mut entry = archive.by_index(idx)?;
            if entry.name().ends_with(".pyi") {
                let output_path = output.join(entry.name());
                if let Some(output_parent) = output_path.parent() {
                    fs::create_dir_all(output_parent)?;
                }
                io::copy(&mut entry, &mut fs::File::create_new(&output_path)?).with_context(
                    || {
                        format!(
                            "Failed to copy {} from {} to {}",
                            entry.name(),
                            wheel.0.display(),
                            output_path.display()
                        )
                    },
                )?;
                found_stubs = true;
            }
        }
    }
    ensure!(
        found_stubs,
        "No auto-generated stubs found for package {}",
        build_context.project.module_name
    );
    Ok(())
}
