mod cargo_toml_rewrite;
mod path_deps;
mod unpack;
mod utils;

use crate::pyproject_toml::{Format, SdistGenerator};
use crate::{BuildContext, ModuleWriter, PyProjectToml, SDistWriter, VirtualWriter};
use anyhow::{Context, Result, bail};
use cargo_metadata::camino::{self, Utf8Path};
use ignore::overrides::Override;
use normpath::PathExt as _;
use path_slash::PathExt as _;
use pyproject_toml::check_pep639_glob;
use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str;
use tracing::{debug, trace, warn};

use self::cargo_toml_rewrite::{
    parse_toml_file, resolve_workspace_inheritance, rewrite_cargo_toml,
    rewrite_cargo_toml_package_field, rewrite_cargo_toml_targets, rewrite_pyproject_toml,
};
pub use self::path_deps::{PathDependency, find_path_deps};
pub use self::unpack::unpack_sdist;
use self::utils::{common_path_prefix, is_compiled_artifact, normalize_path};

// ---------------------------------------------------------------------------
// ManifestAsset: resolve readme / license-file referenced by Cargo.toml
// ---------------------------------------------------------------------------

/// A file (readme or license) referenced by a Cargo.toml manifest field that
/// has been resolved to an absolute path and is ready to be added to the sdist.
#[derive(Debug)]
struct ManifestAsset {
    /// Absolute path to the resolved file on disk.
    source: PathBuf,
    /// Just the filename, used to rewrite the Cargo.toml field so it points
    /// to the copy placed next to the manifest inside the sdist.
    filename: String,
}

/// Resolve a manifest-referenced file (readme or license-file) to an absolute
/// path and validate it is within the allowed root (if given).
///
/// The file is **not** added to the writer here — the caller adds it to the
/// sdist at the appropriate target path.
fn resolve_manifest_asset(
    manifest_dir: &Path,
    field_value: &Path,
    kind: &str,
    allowed_root: Option<&Path>,
) -> Result<ManifestAsset> {
    let file = manifest_dir.join(field_value);
    let abs_file = file
        .normalize()
        .with_context(|| {
            format!(
                "{kind} path `{}` does not exist or is invalid",
                file.display()
            )
        })?
        .into_path_buf();
    if let Some(allowed_root) = allowed_root {
        let allowed_root = allowed_root
            .normalize()
            .with_context(|| {
                format!(
                    "allowed root `{}` does not exist or is invalid",
                    allowed_root.display()
                )
            })?
            .into_path_buf();
        if !abs_file.starts_with(&allowed_root) {
            bail!(
                "{kind} path `{}` resolves outside allowed root `{}`",
                file.display(),
                allowed_root.display()
            );
        }
    }
    let filename = file
        .file_name()
        .and_then(OsStr::to_str)
        .with_context(|| format!("{kind} path `{}` has no filename", file.display()))?
        .to_string();
    Ok(ManifestAsset {
        source: abs_file,
        filename,
    })
}

/// Resolve a manifest asset and immediately add it to the sdist writer.
fn resolve_and_add_manifest_asset(
    writer: &mut VirtualWriter<SDistWriter>,
    manifest_dir: &Path,
    field_value: &Path,
    target_dir: &Path,
    kind: &str,
    allowed_root: Option<&Path>,
) -> Result<ManifestAsset> {
    let asset = resolve_manifest_asset(manifest_dir, field_value, kind, allowed_root)?;
    writer.add_file(target_dir.join(&asset.filename), &asset.source, false)?;
    Ok(asset)
}

// ---------------------------------------------------------------------------
// cargo package --list helper
// ---------------------------------------------------------------------------

/// Run `cargo package --list --allow-dirty` and return the list of files.
fn cargo_package_file_list(manifest_path: &Path) -> Result<Vec<String>> {
    debug!(
        "Getting cargo package file list for {}",
        manifest_path.display()
    );
    let args = ["package", "--list", "--allow-dirty", "--manifest-path"];
    let output = Command::new("cargo")
        .args(args)
        .arg(manifest_path)
        .output()
        .with_context(|| {
            format!(
                "Failed to run `cargo {} {}`",
                args.join(" "),
                manifest_path.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "Failed to query file list from cargo: {}\n--- Manifest path: {}\n--- Stdout:\n{}\n--- Stderr:\n{}",
            output.status,
            manifest_path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    if !output.stderr.is_empty() {
        eprintln!(
            "From `cargo {} {}`:",
            args.join(" "),
            manifest_path.display()
        );
        std::io::stderr().write_all(&output.stderr)?;
    }

    let files = str::from_utf8(&output.stdout)
        .context("Cargo printed invalid utf-8 ಠ_ಠ")?
        .lines()
        .map(String::from)
        .collect();
    Ok(files)
}

// ---------------------------------------------------------------------------
// Core: add a crate's files to the sdist
// ---------------------------------------------------------------------------

/// Options that vary between the root crate and path dependencies when
/// adding crate files to the sdist.
struct AddCrateOptions<'a> {
    /// When true, this is the root crate: rewrite workspace members in
    /// Cargo.toml and skip pyproject.toml (handled separately).
    is_root: bool,
    /// Path deps map — only used for the root crate's workspace.members rewrite.
    known_path_deps: Option<&'a HashMap<String, PathDependency>>,
    /// Path prefixes (relative to the manifest directory) whose files should be
    /// skipped from `cargo package --list` because they are added separately
    /// (e.g. python source directories). See <https://github.com/PyO3/maturin/issues/2383>.
    skip_prefixes: Vec<PathBuf>,
    /// When true, skip writing the (rewritten) Cargo.toml — the workspace
    /// manifest will be added separately with workspace-level rewrites.
    skip_cargo_toml: bool,
    /// When set, the dependency's workspace manifest is outside the sdist root
    /// and workspace-inherited fields must be inlined using these resolved values.
    resolved_package: Option<&'a cargo_metadata::Package>,
}

/// Copies the files of a single crate to the source distribution.
///
/// Runs `cargo package --list --allow-dirty` to obtain a list of files,
/// rewrites `Cargo.toml` as needed, and adds everything to the writer.
fn add_crate_to_source_distribution(
    writer: &mut VirtualWriter<SDistWriter>,
    manifest_path: &Path,
    prefix: &Path,
    readme: Option<&ManifestAsset>,
    license_file: Option<&ManifestAsset>,
    opts: &AddCrateOptions<'_>,
) -> Result<()> {
    let file_list = cargo_package_file_list(manifest_path)?;

    trace!("File list: {:?}", file_list);

    let manifest_dir = manifest_path.parent().unwrap();
    let target_source: Vec<_> = file_list
        .iter()
        .map(|relative_to_manifest| {
            let relative_to_cwd = manifest_dir.join(relative_to_manifest.as_str());
            (relative_to_manifest.as_str(), relative_to_cwd)
        })
        .filter(|(target, source)| {
            if *target == "Cargo.toml.orig" {
                // Skip generated files. See https://github.com/rust-lang/cargo/issues/7938#issuecomment-593280660
                // and https://github.com/PyO3/maturin/issues/449
                false
            } else if *target == "Cargo.toml" {
                // We rewrite Cargo.toml and add it separately
                false
            } else if opts.is_root && *target == "pyproject.toml" {
                // pyproject.toml is handled separately because it has to be put in the root dir
                // of source distribution
                false
            } else if prefix.components().count() == 1 && *target == "pyproject.toml" {
                // Skip pyproject.toml for cases when the root is in a workspace member and both the
                // member and the root have a pyproject.toml.
                debug!(
                    "Skipping potentially non-main {}",
                    prefix.join(target).display()
                );
                false
            } else if !opts.skip_prefixes.is_empty()
                && opts
                    .skip_prefixes
                    .iter()
                    .any(|p| Path::new(target).starts_with(p))
            {
                // Skip files that will be added separately (e.g. python source files
                // that are added by the explicit python source loop).
                // See https://github.com/PyO3/maturin/issues/2383
                debug!(
                    "Skipping {} (will be added separately)",
                    prefix.join(target).display()
                );
                false
            } else if is_compiled_artifact(Path::new(target)) {
                // Technically, `cargo package --list` should handle this,
                // but somehow it doesn't on Alpine Linux running in GitHub Actions.
                // See https://github.com/PyO3/maturin/pull/1255#issuecomment-1308838786
                debug!("Ignoring {}", target);
                false
            } else {
                // Use `is_file` instead of `exists` to work around cargo bug:
                // https://github.com/rust-lang/cargo/issues/16465
                source.is_file() && !writer.exclude(source) && !writer.exclude(prefix.join(target))
            }
        })
        .collect();

    let packaged_files: HashSet<PathBuf> = target_source
        .iter()
        .map(|(target, _)| normalize_path(Path::new(target)))
        .collect();

    // Filter out files that were already added by resolve_and_add_manifest_asset
    // (e.g. readme or license-file from Cargo.toml pointing outside the crate).
    // `cargo package --list` may include a local copy at the same target path,
    // causing a duplicate. See https://github.com/PyO3/maturin/issues/2358
    let target_source: Vec<_> = target_source
        .into_iter()
        .filter(|(target, _)| !writer.contains_target(prefix.join(target)))
        .collect();

    // Write the (potentially rewritten) Cargo.toml
    if !opts.skip_cargo_toml {
        let mut document = parse_toml_file(manifest_path, "Cargo.toml")?;
        rewrite_cargo_toml_package_field(
            &mut document,
            manifest_path,
            "readme",
            readme.map(|a| a.filename.as_str()),
        )?;
        rewrite_cargo_toml_package_field(
            &mut document,
            manifest_path,
            "license-file",
            license_file.map(|a| a.filename.as_str()),
        )?;
        if let Some(known_path_deps) = opts.known_path_deps {
            rewrite_cargo_toml(&mut document, manifest_path, known_path_deps)?;
        }
        if let Some(resolved) = opts.resolved_package {
            resolve_workspace_inheritance(&mut document, resolved);
        }
        rewrite_cargo_toml_targets(&mut document, manifest_path, &packaged_files)?;
        let cargo_toml_path = prefix.join(manifest_path.file_name().unwrap());
        writer.add_bytes(
            cargo_toml_path,
            Some(manifest_path),
            document.to_string().as_bytes(),
            false,
        )?;
    }

    for (target, source) in target_source {
        writer.add_file(prefix.join(target), source, false)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Git-based file listing
// ---------------------------------------------------------------------------

/// Copies git-tracked files to a source distribution.
///
/// Runs `git ls-files -z` to obtain a list of files to package.
fn add_git_tracked_files_to_sdist(
    pyproject_toml_path: &Path,
    writer: &mut VirtualWriter<SDistWriter>,
    prefix: impl AsRef<Path>,
) -> Result<()> {
    let pyproject_dir = pyproject_toml_path.parent().unwrap();
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(pyproject_dir)
        .output()
        .context("Failed to run `git ls-files -z`")?;
    if !output.status.success() {
        bail!(
            "Failed to query file list from git: {}\n--- Project Path: {}\n--- Stdout:\n{}\n--- Stderr:\n{}",
            output.status,
            pyproject_dir.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let prefix = prefix.as_ref();
    let file_paths = str::from_utf8(&output.stdout)
        .context("git printed invalid utf-8 ಠ_ಠ")?
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(Path::new);
    for source in file_paths {
        writer.add_file(prefix.join(source), pyproject_dir.join(source), false)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Cargo-based sdist assembly (decomposed into phases)
// ---------------------------------------------------------------------------

/// Shared context for building a source distribution from cargo packages.
///
/// Groups the common parameters needed when adding crates and path dependencies
/// to the sdist, avoiding excessive argument counts.
struct SdistContext<'a> {
    root_dir: &'a Path,
    workspace_root: &'a Utf8Path,
    workspace_manifest_path: camino::Utf8PathBuf,
    known_path_deps: HashMap<String, PathDependency>,
    sdist_root: PathBuf,
}

/// Resolve sdist_root — the common ancestor of all files that need to be in the sdist.
fn compute_sdist_root(
    workspace_root: &Utf8Path,
    pyproject_toml_path: &Path,
    python_dir: &Path,
    known_path_deps: &HashMap<String, PathDependency>,
) -> Result<PathBuf> {
    let mut sdist_root =
        common_path_prefix(workspace_root.as_std_path(), pyproject_toml_path).unwrap();
    for path_dep in known_path_deps.values() {
        if let Some(prefix) =
            common_path_prefix(&sdist_root, path_dep.manifest_path.parent().unwrap())
        {
            sdist_root = prefix;
        } else {
            bail!("Failed to determine common path prefix of path dependencies");
        }
    }
    // Expand sdist_root to also cover python_dir when python-source points
    // outside the workspace/pyproject directory tree (issue #2202).
    if !python_dir.starts_with(&sdist_root) {
        if let Some(prefix) = common_path_prefix(&sdist_root, python_dir) {
            sdist_root = prefix;
        }
    }
    Ok(sdist_root)
}

/// Determine the outermost project root for computing relative paths inside
/// the sdist. This covers both pyproject.toml and the sdist_root.
fn compute_project_root<'a>(pyproject_toml_path: &'a Path, sdist_root: &'a Path) -> &'a Path {
    let pyproject_root = pyproject_toml_path.parent().unwrap();
    if pyproject_root == sdist_root || pyproject_root.starts_with(sdist_root) {
        sdist_root
    } else {
        assert!(sdist_root.starts_with(pyproject_root));
        pyproject_root
    }
}

/// Add a single path dependency to the sdist.
fn add_path_dep(
    writer: &mut VirtualWriter<SDistWriter>,
    ctx: &SdistContext<'_>,
    name: &str,
    path_dep: &PathDependency,
) -> Result<()> {
    debug!(
        "Adding path dependency: {} at {}",
        name,
        path_dep.manifest_path.display()
    );
    let path_dep_manifest_dir = path_dep.manifest_path.parent().unwrap();
    let relative_path_dep_manifest_dir =
        path_dep_manifest_dir.strip_prefix(&ctx.sdist_root).unwrap();
    // we may need to rewrite workspace Cargo.toml later so don't add it to sdist yet
    let skip_cargo_toml =
        ctx.workspace_manifest_path.as_std_path() == path_dep.manifest_path.as_path();

    // Handle possible relative readme / license-file fields in Cargo.toml
    let target_dir = ctx.root_dir.join(relative_path_dep_manifest_dir);
    let readme = path_dep
        .readme
        .as_ref()
        .map(|readme| {
            resolve_and_add_manifest_asset(
                writer,
                path_dep_manifest_dir,
                readme,
                &target_dir,
                "readme",
                None,
            )
        })
        .transpose()?;
    let license_file = path_dep
        .license_file
        .as_ref()
        .map(|lf| {
            resolve_and_add_manifest_asset(
                writer,
                path_dep_manifest_dir,
                lf,
                &target_dir,
                "license-file",
                Some(&path_dep.workspace_root),
            )
        })
        .transpose()?;

    // Check if the dependency's workspace manifest is outside the sdist root.
    // When it is, we need to inline workspace-inherited fields since we can't
    // include the parent workspace manifest in the sdist.
    let workspace_outside_sdist =
        if path_dep.workspace_root.as_path() != ctx.workspace_root.as_std_path() {
            let path_dep_workspace_manifest = path_dep.workspace_root.join("Cargo.toml");
            path_dep_workspace_manifest
                .strip_prefix(&ctx.sdist_root)
                .is_err()
        } else {
            false
        };

    let resolved_package = if workspace_outside_sdist {
        path_dep.resolved_package.as_ref()
    } else {
        None
    };

    add_crate_to_source_distribution(
        writer,
        &path_dep.manifest_path,
        &ctx.root_dir.join(relative_path_dep_manifest_dir),
        readme.as_ref(),
        license_file.as_ref(),
        &AddCrateOptions {
            is_root: false,
            known_path_deps: None,
            skip_prefixes: Vec::new(),
            skip_cargo_toml,
            resolved_package,
        },
    )
    .with_context(|| {
        format!(
            "Failed to add local dependency {} at {} to the source distribution",
            name,
            path_dep.manifest_path.display()
        )
    })?;

    // Handle different workspace manifest
    if path_dep.workspace_root.as_path() != ctx.workspace_root.as_std_path() {
        let path_dep_workspace_manifest = path_dep.workspace_root.join("Cargo.toml");
        if let Ok(relative_path_dep_workspace_manifest) =
            path_dep_workspace_manifest.strip_prefix(&ctx.sdist_root)
        {
            writer.add_file(
                ctx.root_dir.join(relative_path_dep_workspace_manifest),
                &path_dep_workspace_manifest,
                false,
            )?;
        } else {
            debug!(
                "Skipping workspace manifest at {} (outside sdist root), \
                 workspace-inherited fields have been inlined",
                path_dep_workspace_manifest.display()
            );
        }
    }
    Ok(())
}

/// Add the root crate's files to the sdist.
fn add_main_crate(
    build_context: &BuildContext,
    writer: &mut VirtualWriter<SDistWriter>,
    ctx: &SdistContext<'_>,
) -> Result<PathBuf> {
    let manifest_path = &build_context.manifest_path;
    let abs_manifest_path = manifest_path
        .normalize()
        .with_context(|| {
            format!(
                "manifest path `{}` does not exist or is invalid",
                manifest_path.display()
            )
        })?
        .into_path_buf();
    let abs_manifest_dir = abs_manifest_path.parent().unwrap();
    let main_crate = build_context
        .cargo_metadata
        .root_package()
        .context("Expected cargo to return metadata with root_package")?;
    let relative_main_crate_manifest_dir = manifest_path
        .parent()
        .unwrap()
        .strip_prefix(&ctx.sdist_root)
        .unwrap();

    debug!("Adding the main crate {}", manifest_path.display());

    // Resolve readme / license-file
    let target_dir = ctx.root_dir.join(relative_main_crate_manifest_dir);
    let readme = main_crate
        .readme
        .as_ref()
        .map(|readme| {
            resolve_and_add_manifest_asset(
                writer,
                abs_manifest_dir,
                readme.as_std_path(),
                &target_dir,
                "readme",
                None,
            )
        })
        .transpose()?;
    let license_file = main_crate
        .license_file
        .as_ref()
        .map(|lf| {
            resolve_and_add_manifest_asset(
                writer,
                abs_manifest_dir,
                lf.as_std_path(),
                &target_dir,
                "license-file",
                Some(ctx.workspace_root.as_std_path()),
            )
        })
        .transpose()?;

    // Compute python source directories relative to the manifest directory.
    // When the crate is a workspace member in a subdirectory, `cargo package --list`
    // includes python source files that will also be added by the explicit python
    // source loop (relative to pyproject_dir). We skip them here to avoid duplicates.
    // See https://github.com/PyO3/maturin/issues/2383
    let skip_prefixes: Vec<PathBuf> = if !relative_main_crate_manifest_dir.as_os_str().is_empty() {
        let mut prefixes = Vec::new();
        if let Some(python_module) = build_context.project_layout.python_module.as_ref()
            && let Ok(rel) = python_module.strip_prefix(abs_manifest_dir)
        {
            prefixes.push(rel.to_path_buf());
        }
        for package in &build_context.project_layout.python_packages {
            let package_path = build_context.project_layout.python_dir.join(package);
            if let Ok(rel) = package_path.strip_prefix(abs_manifest_dir)
                && !prefixes.contains(&rel.to_path_buf())
            {
                prefixes.push(rel.to_path_buf());
            }
        }
        prefixes
    } else {
        Vec::new()
    };

    add_crate_to_source_distribution(
        writer,
        manifest_path,
        &ctx.root_dir.join(relative_main_crate_manifest_dir),
        readme.as_ref(),
        license_file.as_ref(),
        &AddCrateOptions {
            is_root: true,
            known_path_deps: Some(&ctx.known_path_deps),
            skip_prefixes,
            skip_cargo_toml: false,
            resolved_package: None,
        },
    )?;

    Ok(abs_manifest_path)
}

/// Add Cargo.lock to the sdist.
fn add_cargo_lock(
    build_context: &BuildContext,
    writer: &mut VirtualWriter<SDistWriter>,
    ctx: &SdistContext<'_>,
    abs_manifest_dir: &Path,
    project_root: &Path,
) -> Result<()> {
    let manifest_cargo_lock_path = abs_manifest_dir.join("Cargo.lock");
    let workspace_cargo_lock = ctx.workspace_root.join("Cargo.lock").into_std_path_buf();
    let cargo_lock_path = if manifest_cargo_lock_path.exists() {
        Some(manifest_cargo_lock_path)
    } else if workspace_cargo_lock.exists() {
        Some(workspace_cargo_lock)
    } else {
        None
    };
    let cargo_lock_required =
        build_context.cargo_options.locked || build_context.cargo_options.frozen;

    if let Some(cargo_lock_path) = cargo_lock_path {
        let relative_cargo_lock = cargo_lock_path.strip_prefix(project_root).unwrap();
        writer.add_file(
            ctx.root_dir.join(relative_cargo_lock),
            &cargo_lock_path,
            false,
        )?;
    } else if cargo_lock_required {
        bail!("Cargo.lock is required by `--locked`/`--frozen` but it's not found.");
    } else {
        eprintln!(
            "⚠️  Warning: Cargo.lock is not found, it is recommended \
            to include it in the source distribution"
        );
    }
    Ok(())
}

/// Add the workspace Cargo.toml (when the crate is a workspace member).
fn add_workspace_manifest(
    writer: &mut VirtualWriter<SDistWriter>,
    ctx: &SdistContext<'_>,
    manifest_path: &Path,
    abs_manifest_dir: &Path,
    project_root: &Path,
) -> Result<()> {
    // Without the workspace Cargo.toml, cargo can't resolve workspace-level deps.
    // Note: when a crate is `exclude`d from a workspace, `cargo metadata` reports
    // the crate's own directory as `workspace_root`, so this check correctly
    // skips adding the parent workspace Cargo.toml for excluded crates.
    //
    // We normalize workspace_root to match abs_manifest_dir (also normalized) so
    // that symlinks or .. components don't cause a false positive.
    let normalized_workspace_root = ctx
        .workspace_root
        .as_std_path()
        .normalize()
        .map(|p| p.into_path_buf())
        .unwrap_or_else(|_| ctx.workspace_root.as_std_path().to_path_buf());
    let is_in_workspace = normalized_workspace_root != abs_manifest_dir;
    if !is_in_workspace {
        return Ok(());
    }

    let relative_workspace_cargo_toml = ctx
        .workspace_manifest_path
        .as_std_path()
        .strip_prefix(project_root)
        .unwrap();

    // Collect all crates that must remain in `workspace.members`:
    // the known path dependencies plus the main Python binding crate itself.
    let mut deps_to_keep = ctx.known_path_deps.clone();
    let main_member_name = abs_manifest_dir
        .strip_prefix(ctx.workspace_root)
        .unwrap()
        .to_slash()
        .unwrap()
        .to_string();
    deps_to_keep.insert(
        main_member_name,
        PathDependency {
            manifest_path: manifest_path.to_path_buf(),
            workspace_root: ctx.workspace_root.as_std_path().to_path_buf(),
            readme: None,
            license_file: None,
            resolved_package: None,
        },
    );

    // Rewrite workspace Cargo.toml to only include relevant members.
    let mut document = parse_toml_file(ctx.workspace_manifest_path.as_std_path(), "Cargo.toml")?;
    rewrite_cargo_toml(
        &mut document,
        ctx.workspace_manifest_path.as_std_path(),
        &deps_to_keep,
    )?;

    // When the workspace root Cargo.toml is also a [package] (virtual
    // workspaces don't have one), the package's source files are typically
    // not included in the sdist. Strip the [package] section so cargo
    // treats it as a virtual workspace.
    let workspace_root_is_path_dep = ctx
        .known_path_deps
        .values()
        .any(|dep| dep.manifest_path.as_path() == ctx.workspace_manifest_path.as_std_path());
    if !workspace_root_is_path_dep && document.contains_key("package") {
        debug!(
            "Stripping [package] from workspace Cargo.toml at {} (source files not in sdist)",
            ctx.workspace_manifest_path
        );
        let package_level_keys: Vec<String> = document
            .as_table()
            .iter()
            .filter(|(key, _)| !matches!(&**key, "workspace" | "profile" | "patch" | "replace"))
            .map(|(key, _)| key.to_string())
            .collect();
        for key in &package_level_keys {
            document.remove(key);
        }
    }

    writer.add_bytes(
        ctx.root_dir.join(relative_workspace_cargo_toml),
        Some(ctx.workspace_manifest_path.as_std_path()),
        document.to_string().as_bytes(),
        false,
    )?;
    Ok(())
}

/// Add pyproject.toml to the sdist (rewriting paths if necessary).
fn add_pyproject_toml(
    build_context: &BuildContext,
    writer: &mut VirtualWriter<SDistWriter>,
    ctx: &SdistContext<'_>,
    pyproject_toml_path: &Path,
    relative_main_crate_manifest_dir: &Path,
    project_root: &Path,
) -> Result<()> {
    let pyproject_dir = pyproject_toml_path.parent().unwrap();
    if pyproject_dir != ctx.sdist_root {
        let python_dir = &build_context.project_layout.python_dir;
        // Compute python-source relative to pyproject_dir.  When python_dir is
        // outside pyproject_dir, compute the path relative to project_root instead.
        let relative_python_source = if python_dir != pyproject_dir {
            python_dir
                .strip_prefix(pyproject_dir)
                .or_else(|_| python_dir.strip_prefix(project_root))
                .ok()
                .map(|p| p.to_path_buf())
        } else {
            None
        };
        let rewritten_pyproject_toml = rewrite_pyproject_toml(
            pyproject_toml_path,
            &relative_main_crate_manifest_dir.join("Cargo.toml"),
            relative_python_source.as_deref(),
        )?;
        writer.add_bytes(
            ctx.root_dir.join("pyproject.toml"),
            Some(pyproject_toml_path),
            rewritten_pyproject_toml.as_bytes(),
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
fn add_python_sources(
    build_context: &BuildContext,
    writer: &mut VirtualWriter<SDistWriter>,
    root_dir: &Path,
    pyproject_dir: &Path,
    project_root: &Path,
) -> Result<()> {
    let mut python_packages = Vec::new();
    if let Some(python_module) = build_context.project_layout.python_module.as_ref() {
        trace!("Resolved python module: {}", python_module.display());
        python_packages.push(python_module.to_path_buf());
    }
    for package in &build_context.project_layout.python_packages {
        let package_path = build_context.project_layout.python_dir.join(package);
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
                .strip_prefix(pyproject_dir)
                .or_else(|_| source.strip_prefix(project_root))
                .with_context(|| {
                    format!(
                        "Python source file `{}` is outside both pyproject dir `{}` and project root `{}`",
                        source.display(),
                        pyproject_dir.display(),
                        project_root.display(),
                    )
                })?;
            if !source.is_dir() {
                writer.add_file(root_dir.join(relative), &source, false)?;
            }
        }
    }
    Ok(())
}

/// Orchestrates adding all cargo package files to the sdist.
///
/// Delegates to focused helpers for each phase:
/// 1. Path dependencies
/// 2. Main crate
/// 3. Cargo.lock
/// 4. Workspace manifest
/// 5. pyproject.toml
/// 6. Python sources
fn add_cargo_package_files_to_sdist(
    build_context: &BuildContext,
    pyproject_toml_path: &Path,
    writer: &mut VirtualWriter<SDistWriter>,
    root_dir: &Path,
) -> Result<()> {
    let manifest_path = &build_context.manifest_path;
    let workspace_root = &build_context.cargo_metadata.workspace_root;
    let workspace_manifest_path = workspace_root.join("Cargo.toml");

    let known_path_deps = find_path_deps(&build_context.cargo_metadata)?;
    debug!(
        "Found path dependencies: {:?}",
        known_path_deps.keys().collect::<Vec<_>>()
    );

    let sdist_root = compute_sdist_root(
        workspace_root,
        pyproject_toml_path,
        &build_context.project_layout.python_dir,
        &known_path_deps,
    )?;
    debug!("Found sdist root: {}", sdist_root.display());

    let ctx = SdistContext {
        root_dir,
        workspace_root,
        workspace_manifest_path,
        known_path_deps,
        sdist_root,
    };

    // 1. Add local path dependencies
    for (name, path_dep) in ctx.known_path_deps.iter() {
        add_path_dep(writer, &ctx, name, path_dep)
            .with_context(|| format!("Failed to add path dependency {name}"))?;
    }

    // 2. Add the main crate
    let abs_manifest_path = add_main_crate(build_context, writer, &ctx)?;
    let abs_manifest_dir = abs_manifest_path.parent().unwrap();
    let relative_main_crate_manifest_dir = manifest_path
        .parent()
        .unwrap()
        .strip_prefix(&ctx.sdist_root)
        .unwrap();

    // Compute the project root (needed by several subsequent phases)
    let project_root = compute_project_root(pyproject_toml_path, &ctx.sdist_root);
    let pyproject_dir = pyproject_toml_path.parent().unwrap();

    // 3. Add Cargo.lock
    add_cargo_lock(build_context, writer, &ctx, abs_manifest_dir, project_root)?;

    // 4. Add workspace Cargo.toml (if applicable)
    add_workspace_manifest(writer, &ctx, manifest_path, abs_manifest_dir, project_root)?;

    // 5. Add pyproject.toml
    add_pyproject_toml(
        build_context,
        writer,
        &ctx,
        pyproject_toml_path,
        relative_main_crate_manifest_dir,
        project_root,
    )?;

    // 6. Add python source files
    add_python_sources(build_context, writer, root_dir, pyproject_dir, project_root)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Creates a source distribution, packing the root crate and all local dependencies
///
/// The source distribution format is specified in
/// [PEP 517 under "build_sdist"](https://www.python.org/dev/peps/pep-0517/#build-sdist)
/// and in
/// https://packaging.python.org/specifications/source-distribution-format/#source-distribution-file-format
pub fn source_distribution(
    build_context: &BuildContext,
    pyproject: &PyProjectToml,
    excludes: Override,
) -> Result<PathBuf> {
    let pyproject_toml_path = build_context
        .pyproject_toml_path
        .normalize()
        .with_context(|| {
            format!(
                "pyproject.toml path `{}` does not exist or is invalid",
                build_context.pyproject_toml_path.display()
            )
        })?
        .into_path_buf();

    let source_date_epoch: Option<u64> =
        env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|var| match var.parse() {
                Err(_) => {
                    warn!("SOURCE_DATE_EPOCH is malformed, ignoring");
                    None
                }
                Ok(val) => Some(val),
            });

    let metadata24 = &build_context.metadata24;
    let writer = SDistWriter::new(&build_context.out, metadata24, source_date_epoch)?;
    let mut writer = VirtualWriter::new(writer, excludes);
    let root_dir = PathBuf::from(format!(
        "{}-{}",
        &metadata24.get_distribution_escaped(),
        &metadata24.get_version_escaped()
    ));

    match pyproject.sdist_generator() {
        SdistGenerator::Cargo => add_cargo_package_files_to_sdist(
            build_context,
            &pyproject_toml_path,
            &mut writer,
            &root_dir,
        )?,
        SdistGenerator::Git => {
            add_git_tracked_files_to_sdist(&pyproject_toml_path, &mut writer, &root_dir)?
        }
    }

    let pyproject_dir = pyproject_toml_path.parent().unwrap();
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

    let python_dir = &build_context.project_layout.python_dir;

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

    let pkg_info = root_dir.join("PKG-INFO");
    writer.add_bytes(
        &pkg_info,
        None,
        metadata24.to_file_contents()?.as_bytes(),
        false,
    )?;

    let source_distribution_path = writer.finish(&pkg_info)?;

    eprintln!(
        "📦 Built source distribution to {}",
        source_distribution_path.display()
    );

    Ok(source_distribution_path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err as fs;
    use ignore::overrides::Override;
    use pep440_rs::Version;
    use std::str::FromStr;
    use tempfile::TempDir;

    use crate::Metadata24;

    #[test]
    fn test_resolve_manifest_asset_rejects_license_outside_allowed_root() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_root = temp_dir.path().join("workspace");
        let manifest_dir = workspace_root.join("crate");
        let out_dir = temp_dir.path().join("out");
        fs::create_dir_all(manifest_dir.join("src")).unwrap();
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(manifest_dir.join("src/lib.rs"), "").unwrap();
        fs::write(temp_dir.path().join("SECRET_LICENSE"), "secret").unwrap();

        let err = resolve_manifest_asset(
            &manifest_dir,
            Path::new("../../SECRET_LICENSE"),
            "license-file",
            Some(&workspace_root),
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("outside allowed root"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn test_resolve_and_add_manifest_asset_rejects_license_outside_allowed_root() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_root = temp_dir.path().join("workspace");
        let manifest_dir = workspace_root.join("crate");
        let out_dir = temp_dir.path().join("out");
        fs::create_dir_all(manifest_dir.join("src")).unwrap();
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(manifest_dir.join("src/lib.rs"), "").unwrap();
        fs::write(temp_dir.path().join("SECRET_LICENSE"), "secret").unwrap();

        let metadata = Metadata24::new("test-pkg".to_string(), Version::from_str("1.0.0").unwrap());
        let sdist_writer = SDistWriter::new(&out_dir, &metadata, None).unwrap();
        let mut writer = VirtualWriter::new(sdist_writer, Override::empty());

        let err = resolve_and_add_manifest_asset(
            &mut writer,
            &manifest_dir,
            Path::new("../../SECRET_LICENSE"),
            &Path::new("pkg-1.0.0").join("crate"),
            "license-file",
            Some(&workspace_root),
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("outside allowed root"),
            "unexpected error: {err:#}"
        );
    }
}
