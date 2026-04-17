use crate::pyproject_toml::Format;
use crate::{ModuleWriter, PyProjectToml, SDistWriter, VirtualWriter};
use anyhow::{Context, Result};
use path_slash::PathExt as _;
use pyproject_toml::check_pep639_glob;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{debug, trace};

use super::SdistContext;
use super::cargo_toml_rewrite::parse_toml_file;
use super::utils::is_compiled_artifact;

/// Add pyproject.toml to the sdist (rewriting paths if necessary).
pub(super) fn add_pyproject_toml(
    writer: &mut VirtualWriter<SDistWriter>,
    ctx: &SdistContext<'_>,
    pyproject_toml_path: &Path,
) -> Result<()> {
    if ctx.pyproject_dir != ctx.sdist_root {
        let python_dir = &ctx.project.project_layout.python_dir;
        // Compute python-source relative to pyproject_dir.  When python_dir is
        // outside pyproject_dir, compute the path relative to project_root instead.
        let relative_python_source = if python_dir != &ctx.pyproject_dir {
            python_dir
                .strip_prefix(&ctx.pyproject_dir)
                .or_else(|_| python_dir.strip_prefix(&ctx.project_root))
                .ok()
                .map(|p| p.to_path_buf())
        } else {
            None
        };
        let rewritten = rewrite_pyproject_toml(
            pyproject_toml_path,
            &ctx.relative_main_crate_manifest_dir.join("Cargo.toml"),
            relative_python_source.as_deref(),
        )?;
        writer.add_bytes(
            ctx.root_dir.join("pyproject.toml"),
            Some(pyproject_toml_path),
            rewritten.as_bytes(),
            false,
        )?;
    } else {
        writer.add_file(
            ctx.root_dir.join("pyproject.toml"),
            pyproject_toml_path,
            false,
        )?;
    }
    Ok(())
}

/// Add python source files to the sdist.
pub(super) fn add_python_sources(
    writer: &mut VirtualWriter<SDistWriter>,
    ctx: &SdistContext<'_>,
) -> Result<()> {
    let project = ctx.project;
    let mut python_packages = Vec::new();
    if let Some(python_module) = project.project_layout.python_module.as_ref() {
        trace!("Resolved python module: {}", python_module.display());
        python_packages.push(python_module.to_path_buf());
    }
    for package in &project.project_layout.python_packages {
        let package_path = project.project_layout.python_dir.join(package);
        if python_packages.contains(&package_path) {
            continue;
        }
        trace!("Resolved python package: {}", package_path.display());
        python_packages.push(package_path);
    }

    for package in python_packages {
        for entry in ignore::Walk::new(package) {
            let source = entry?.into_path();
            if is_compiled_artifact(&source) {
                debug!("Ignoring {}", source.display());
                continue;
            }
            // When python-source points outside pyproject_dir, strip from
            // project_root instead (issue #2202).
            let relative = source
                .strip_prefix(&ctx.pyproject_dir)
                .or_else(|_| source.strip_prefix(&ctx.project_root))
                .with_context(|| {
                    format!(
                        "Python source file `{}` is outside both pyproject dir `{}` and project root `{}`",
                        source.display(),
                        ctx.pyproject_dir.display(),
                        ctx.project_root.display(),
                    )
                })?;
            if !source.is_dir() {
                writer.add_file(ctx.root_dir.join(relative), &source, false)?;
            }
        }
    }
    Ok(())
}

/// Add readme, license files, and include patterns from pyproject.toml metadata.
///
/// This covers files referenced by `[project]` fields (readme, license,
/// license-files with PEP 639 glob handling) as well as explicit `include`
/// patterns from `[tool.maturin]`.  Files already present in the writer
/// (e.g. from Cargo.toml metadata) are skipped to avoid duplicates.
pub(super) fn add_pyproject_metadata(
    writer: &mut VirtualWriter<SDistWriter>,
    pyproject: &PyProjectToml,
    pyproject_dir: &Path,
    root_dir: &Path,
    python_dir: &Path,
) -> Result<()> {
    // Add readme, license from pyproject.toml
    // Skip if already added (e.g. from Cargo.toml metadata) to avoid duplicates.
    // See https://github.com/PyO3/maturin/issues/2358
    if let Some(project) = pyproject.project.as_ref() {
        if let Some(pyproject_toml::ReadMe::RelativePath(readme)) = project.readme.as_ref() {
            let target = root_dir.join(readme);
            if !writer.contains_target(&target) {
                writer.add_file(target, pyproject_dir.join(readme), false)?;
            }
        }
        if let Some(pyproject_toml::License::File { file }) = project.license.as_ref() {
            let target = root_dir.join(file);
            if !writer.contains_target(&target) {
                writer.add_file(target, pyproject_dir.join(file), false)?;
            }
        }
        if let Some(license_files) = &project.license_files {
            let escaped_pyproject_dir =
                PathBuf::from(glob::Pattern::escape(pyproject_dir.to_str().unwrap()));
            let mut seen = HashSet::new();
            for license_glob in license_files {
                check_pep639_glob(license_glob)?;
                for license_path in
                    glob::glob(&escaped_pyproject_dir.join(license_glob).to_string_lossy())?
                {
                    let license_path = license_path?;
                    if !license_path.is_file() {
                        continue;
                    }
                    let license_path = license_path
                        .strip_prefix(pyproject_dir)
                        .expect("matched path starts with glob root")
                        .to_path_buf();
                    if seen.insert(license_path.clone()) {
                        debug!("Including license file `{}`", license_path.display());
                        writer.add_file(
                            root_dir.join(&license_path),
                            pyproject_dir.join(&license_path),
                            false,
                        )?;
                    }
                }
            }
        }
    }

    if let Some(glob_patterns) = pyproject.include() {
        for pattern in glob_patterns
            .iter()
            .filter_map(|glob_pattern| glob_pattern.targets(Format::Sdist))
        {
            eprintln!("📦 Including files matching \"{pattern}\"");
            let matches = crate::module_writer::glob::resolve_include_matches(
                pattern,
                Format::Sdist,
                pyproject_dir,
                python_dir,
            )?;
            for m in matches {
                writer.add_file(root_dir.join(&m.target), m.source, false)?;
            }
        }
    }

    Ok(())
}

/// Rewrite `pyproject.toml` paths for the sdist layout.
///
/// When `pyproject.toml` lives inside the Cargo workspace root (not at the
/// sdist root), we update `tool.maturin.manifest-path` and optionally
/// `tool.maturin.python-source` so they resolve correctly from the new
/// relative position inside the archive.
fn rewrite_pyproject_toml(
    pyproject_toml_path: &Path,
    relative_manifest_path: &Path,
    relative_python_source: Option<&Path>,
) -> Result<String> {
    let mut data = parse_toml_file(pyproject_toml_path, "pyproject.toml")?;
    let tool = data
        .entry("tool")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_like_mut()
        .with_context(|| {
            format!(
                "`[tool]` must be a table in {}",
                pyproject_toml_path.display()
            )
        })?;
    let maturin = tool
        .entry("maturin")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_like_mut()
        .with_context(|| {
            format!(
                "`[tool.maturin]` must be a table in {}",
                pyproject_toml_path.display()
            )
        })?;

    maturin.remove("manifest-path");
    let manifest_path_str = relative_manifest_path.to_slash().with_context(|| {
        format!(
            "manifest-path `{}` is not valid UTF-8",
            relative_manifest_path.display()
        )
    })?;
    maturin.insert(
        "manifest-path",
        toml_edit::value(manifest_path_str.as_ref()),
    );

    if let Some(python_source) = relative_python_source {
        maturin.remove("python-source");
        let python_source_str = python_source.to_slash().with_context(|| {
            format!(
                "python-source path `{}` is not valid UTF-8",
                python_source.display()
            )
        })?;
        maturin.insert(
            "python-source",
            toml_edit::value(python_source_str.as_ref()),
        );
    }

    Ok(data.to_string())
}
