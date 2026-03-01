use crate::pyproject_toml::{Format, SdistGenerator};
use crate::{BuildContext, ModuleWriter, PyProjectToml, SDistWriter, VirtualWriter};
use anyhow::{Context, Result, bail};
use cargo_metadata::camino::{self, Utf8Path};
use cargo_metadata::{Metadata, MetadataCommand, PackageId};
use fs_err as fs;
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
use toml_edit::DocumentMut;
use tracing::{debug, trace, warn};

/// Unpacks an sdist tarball into a temporary directory and returns the path
/// to the Cargo.toml and pyproject.toml inside it, along with the tempdir
/// handle (which must be kept alive for the duration of the build).
///
/// The Cargo.toml path is resolved by checking `[tool.maturin.manifest-path]`
/// in the sdist's `pyproject.toml`, falling back to `Cargo.toml` at the
/// sdist root directory.
pub fn unpack_sdist(sdist_path: &Path) -> Result<(tempfile::TempDir, PathBuf, PathBuf)> {
    let tmp = tempfile::tempdir().context("Failed to create temporary directory")?;
    let gz = flate2::read::GzDecoder::new(
        fs::File::open(sdist_path)
            .with_context(|| format!("Failed to open sdist {}", sdist_path.display()))?,
    );
    let mut archive = tar::Archive::new(gz);
    archive
        .unpack(tmp.path())
        .context("Failed to unpack source distribution")?;

    // The sdist contains a single top-level directory named <name>-<version>.
    let entries: Vec<_> = fs::read_dir(tmp.path())
        .context("Failed to read unpacked sdist directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    let top_dir = match entries.len() {
        // Canonicalize to resolve symlinks (e.g. /var -> /private/var on macOS).
        // Without this, `project_root` and `python_dir` may disagree after
        // `normalize()` is applied to only some paths, causing python source
        // files to be silently excluded from wheels.
        1 => dunce::canonicalize(entries[0].path()).unwrap_or_else(|_| entries[0].path()),
        n => bail!(
            "Expected exactly one top-level directory in sdist, found {}",
            n
        ),
    };

    // Resolve the Cargo.toml path: check pyproject.toml for [tool.maturin.manifest-path],
    // otherwise default to Cargo.toml at the sdist root.
    let pyproject_file = top_dir.join("pyproject.toml");
    let cargo_toml = if pyproject_file.is_file() {
        let pyproject = PyProjectToml::new(&pyproject_file)?;
        if let Some(manifest_path) = pyproject.manifest_path() {
            top_dir.join(manifest_path)
        } else {
            top_dir.join("Cargo.toml")
        }
    } else {
        top_dir.join("Cargo.toml")
    };
    if !cargo_toml.exists() {
        bail!(
            "Cargo.toml not found in unpacked sdist at {}",
            cargo_toml.display()
        );
    }
    Ok((tmp, cargo_toml, pyproject_file))
}

/// Path dependency information.
/// It may be in a different workspace than the root crate.
///
/// ```toml
/// [dependencies]
/// foo = { path = "path/to/foo" }
/// ```
#[derive(Debug, Clone)]
pub struct PathDependency {
    /// `Cargo.toml` path of the path dependency
    manifest_path: PathBuf,
    /// workspace root of the path dependency
    workspace_root: PathBuf,
    /// readme path of the path dependency
    readme: Option<PathBuf>,
    /// license-file path of the path dependency
    license_file: Option<PathBuf>,
}

/// Returns `true` if the file extension indicates a compiled artifact
/// that should be excluded from the source distribution.
fn is_compiled_artifact(path: &Path) -> bool {
    matches!(path.extension(), Some(ext) if ext == "pyc" || ext == "pyd" || ext == "so")
}

/// Resolve a file path relative to a manifest directory and add it to the sdist
/// next to its `Cargo.toml` to avoid collisions between crates using files
/// higher up the file tree.
///
/// `kind` is used in error messages (e.g. "readme", "license-file").
/// If `allowed_root` is set, the resolved path must be under it.
/// Returns the absolute path of the file.
fn resolve_and_add_file(
    writer: &mut VirtualWriter<SDistWriter>,
    file: &Path,
    manifest_dir: &Path,
    target_dir: &Path,
    kind: &str,
    allowed_root: Option<&Path>,
) -> Result<PathBuf> {
    let file = manifest_dir.join(file);
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
        .with_context(|| format!("{kind} path `{}` has no filename", file.display()))?;
    writer.add_file(target_dir.join(filename), &abs_file, false)?;
    Ok(abs_file)
}

/// Resolve a readme path relative to a manifest directory and add it to the sdist.
/// See [`resolve_and_add_file`] for details.
fn resolve_and_add_readme(
    writer: &mut VirtualWriter<SDistWriter>,
    readme: &Path,
    manifest_dir: &Path,
    target_dir: &Path,
) -> Result<PathBuf> {
    resolve_and_add_file(writer, readme, manifest_dir, target_dir, "readme", None)
}

fn parse_toml_file(path: &Path, kind: &str) -> Result<toml_edit::DocumentMut> {
    let text = fs::read_to_string(path)?;
    let document = text.parse::<toml_edit::DocumentMut>().context(format!(
        "Failed to parse {} at {}",
        kind,
        path.display()
    ))?;
    Ok(document)
}

/// Rewrite Cargo.toml to only retain path dependencies that are actually used
///
/// We only want to add path dependencies that are actually used
/// to reduce the size of the source distribution.
fn rewrite_cargo_toml(
    document: &mut DocumentMut,
    manifest_path: &Path,
    known_path_deps: &HashMap<String, PathDependency>,
) -> Result<()> {
    debug!(
        "Rewriting Cargo.toml `workspace.members` at {}",
        manifest_path.display()
    );
    // Update workspace members
    if let Some(workspace) = document.get_mut("workspace").and_then(|x| x.as_table_mut())
        && let Some(members) = workspace.get_mut("members").and_then(|x| x.as_array())
    {
        if known_path_deps.is_empty() {
            // Remove workspace members when there isn't any path dep
            workspace.remove("members");
            if workspace.is_empty() {
                // Remove workspace all together if it's empty
                document.remove("workspace");
            }
        } else {
            // Build a set of relative directory paths (from workspace root) for
            // all known path dependencies. Workspace `members` entries are
            // directory paths, not crate names, so we must compare against the
            // actual directory of each dependency rather than its name.
            let relative_dep_dirs: HashSet<String> = known_path_deps
                .values()
                .filter_map(|path_dep| {
                    let manifest_rel = path_dep
                        .manifest_path
                        .strip_prefix(&path_dep.workspace_root)
                        .ok()?;
                    // Strip the trailing `Cargo.toml` to get the directory
                    manifest_rel.parent().and_then(|p| p.to_slash()).map(|s| {
                        if s.is_empty() {
                            ".".into()
                        } else {
                            s.into_owned()
                        }
                    })
                })
                .collect();

            let mut new_members = toml_edit::Array::new();
            for member in members {
                if let toml_edit::Value::String(s) = member {
                    let member_path = s.value();
                    // See https://github.com/rust-lang/cargo/blob/0de91c89e6479016d0ed8719fdc2947044335b36/src/cargo/util/restricted_names.rs#L119-L122
                    let is_glob_pattern = member_path.contains(['*', '?', '[', ']']);
                    if is_glob_pattern {
                        let pattern = glob::Pattern::new(member_path).with_context(|| {
                            format!(
                                "Invalid `workspace.members` glob pattern: {} in {}",
                                member_path,
                                manifest_path.display()
                            )
                        })?;
                        if relative_dep_dirs.iter().any(|dir| pattern.matches(dir)) {
                            new_members.push(member_path);
                        }
                    } else if relative_dep_dirs.contains(member_path) {
                        new_members.push(member_path);
                    }
                }
            }
            if !new_members.is_empty() {
                workspace["members"] = toml_edit::value(new_members);
            } else {
                workspace.remove("members");
            }
        }
    }

    // Remove `default-members` to avoid build failures when some entries
    // are not included in the sdist. Without `default-members`, Cargo
    // treats all `members` as defaults, which is the correct behavior
    // for source distributions. See https://github.com/PyO3/maturin/issues/2046
    if let Some(workspace) = document.get_mut("workspace").and_then(|x| x.as_table_mut()) {
        workspace.remove("default-members");
    }

    Ok(())
}

fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut components = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop if there's a normal component to cancel out
                match components.last() {
                    Some(Component::Normal(_)) => {
                        components.pop();
                    }
                    _ => {
                        // Keep the ParentDir if path is empty or last is also ParentDir
                        components.push(component);
                    }
                }
            }
            _ => components.push(component),
        }
    }

    components.iter().collect()
}

// Strip targets whose source files are excluded from the sdist, matching Cargo's
// behavior when `package.include`/`package.exclude` or tool.maturin excludes remove them.
fn rewrite_cargo_toml_targets(
    document: &mut DocumentMut,
    manifest_path: &Path,
    packaged_files: &HashSet<PathBuf>,
) -> Result<()> {
    debug!(
        "Rewriting Cargo.toml build targets at {}",
        manifest_path.display()
    );

    let manifest_dir = manifest_path.parent().unwrap();
    let package_name = document
        .get("package")
        .and_then(|item| item.as_table())
        .and_then(|table| table.get("name"))
        .and_then(|item| item.as_str())
        .map(str::to_string);

    // We need to normalize paths without accessing the filesystem (which might not match
    // the manifest context) and without resolving symlinks. This matches `cargo package --list`
    // behavior which outputs normalized paths.
    let normalize = |path: &Path| -> PathBuf {
        let path = if path.is_absolute() {
            path.strip_prefix(manifest_dir).unwrap_or(path)
        } else {
            path
        };
        normalize_path(path)
    };

    let has_packaged_path =
        |paths: &[PathBuf]| -> bool { paths.iter().any(|path| packaged_files.contains(path)) };

    // Cargo's implicit target path rules when `path` is not set:
    // - lib: src/lib.rs
    // - bin: src/bin/<name>.rs or src/bin/<name>/main.rs (src/main.rs only for implicit default bin)
    // - example/test/bench: <dir>/<name>.rs or <dir>/<name>/main.rs
    let candidate_paths_for_target =
        |kind: &str, name: Option<&str>, path: Option<&str>, package_name: Option<&str>| {
            if let Some(path) = path {
                return vec![normalize(Path::new(path))];
            }

            let name = name.or(package_name);
            match (kind, name) {
                ("lib", _) => vec![normalize(Path::new("src/lib.rs"))],
                ("bin", Some(name)) => {
                    vec![
                        normalize(Path::new(&format!("src/bin/{name}.rs"))),
                        normalize(Path::new(&format!("src/bin/{name}/main.rs"))),
                    ]
                }
                ("bin", None) => vec![normalize(Path::new("src/main.rs"))],
                ("example", Some(name)) => vec![
                    normalize(Path::new(&format!("examples/{name}.rs"))),
                    normalize(Path::new(&format!("examples/{name}/main.rs"))),
                ],
                ("test", Some(name)) => vec![
                    normalize(Path::new(&format!("tests/{name}.rs"))),
                    normalize(Path::new(&format!("tests/{name}/main.rs"))),
                ],
                ("bench", Some(name)) => vec![
                    normalize(Path::new(&format!("benches/{name}.rs"))),
                    normalize(Path::new(&format!("benches/{name}/main.rs"))),
                ],
                _ => Vec::new(),
            }
        };

    let package_name = package_name.as_deref();

    let mut drop_lib = false;
    if let Some(lib) = document.get("lib").and_then(|item| item.as_table()) {
        let name = lib.get("name").and_then(|item| item.as_str());
        let path = lib.get("path").and_then(|item| item.as_str());
        let candidates = candidate_paths_for_target("lib", name, path, package_name);
        if !candidates.is_empty() && !has_packaged_path(&candidates) {
            debug!(
                "Stripping [lib] target {:?} from {}",
                name.or(path),
                manifest_path.display()
            );
            drop_lib = true;
        }
    }

    if drop_lib {
        document.remove("lib");
    }

    let mut removed_bins = Vec::new();
    for (key, kind) in [
        ("bin", "bin"),
        ("example", "example"),
        ("test", "test"),
        ("bench", "bench"),
    ] {
        if let Some(targets) = document
            .get_mut(key)
            .and_then(|item| item.as_array_of_tables_mut())
        {
            let mut idx = 0;
            while idx < targets.len() {
                let target = targets.get(idx).unwrap();
                let name = target.get("name").and_then(|item| item.as_str());
                let path = target.get("path").and_then(|item| item.as_str());
                let candidates = candidate_paths_for_target(kind, name, path, package_name);
                if !candidates.is_empty() && !has_packaged_path(&candidates) {
                    debug!(
                        "Stripping {key} target {:?} from {}",
                        name.or(path),
                        manifest_path.display()
                    );
                    if kind == "bin"
                        && let Some(name) = name
                    {
                        removed_bins.push(name.to_string());
                    }
                    targets.remove(idx);
                } else {
                    idx += 1;
                }
            }
            if targets.is_empty() {
                document.remove(key);
            }
        }
    }

    // If we removed any binaries, we must check if they were the `default-run` target.
    // If so, we remove `default-run` to prevent `cargo run` from failing with a missing target.
    if !removed_bins.is_empty()
        && let Some(package) = document
            .get_mut("package")
            .and_then(|item| item.as_table_mut())
        && let Some(default_run) = package.get("default-run").and_then(|item| item.as_str())
        && removed_bins.iter().any(|name| name == default_run)
    {
        debug!(
            "Stripping [package.default-run] target {:?} from {}",
            default_run,
            manifest_path.display()
        );
        package.remove("default-run");
    }

    Ok(())
}

/// Rewrite `Cargo.toml` to find the readme in the same directory.
///
/// `package.readme` may point to any point above the package, so when we move the directory, but
/// keep the readme position, we could get different readme files at the same archive location.
/// Putting the readme in the same directory as the `Cargo.toml` prevents this.
fn rewrite_cargo_toml_readme(
    document: &mut DocumentMut,
    manifest_path: &Path,
    readme_name: Option<&str>,
) -> Result<()> {
    debug!(
        "Rewriting Cargo.toml `package.readme` at {}",
        manifest_path.display()
    );

    if let Some(readme_name) = readme_name {
        let project = document.get_mut("package").with_context(|| {
            format!(
                "Missing `[package]` table in Cargo.toml with readme at {}",
                manifest_path.display()
            )
        })?;
        project["readme"] = toml_edit::value(readme_name);
    }
    Ok(())
}

/// Rewrite `Cargo.toml` to find the license file in the same directory.
///
/// `package.license-file` may point above the package (e.g. workspace-level license),
/// so when we flatten the directory structure in the sdist, the path needs updating.
/// This mirrors what [`rewrite_cargo_toml_readme`] does for readmes.
fn rewrite_cargo_toml_license_file(
    document: &mut DocumentMut,
    manifest_path: &Path,
    license_file_name: Option<&str>,
) -> Result<()> {
    debug!(
        "Rewriting Cargo.toml `package.license-file` at {}",
        manifest_path.display()
    );

    if let Some(license_file_name) = license_file_name {
        let project = document.get_mut("package").with_context(|| {
            format!(
                "Missing `[package]` table in Cargo.toml with license-file at {}",
                manifest_path.display()
            )
        })?;
        project["license-file"] = toml_edit::value(license_file_name);
    }
    Ok(())
}

/// When `pyproject.toml` is inside the Cargo workspace root,
/// we need to update `tool.maturin.manifest-path` and `tool.maturin.python-source`
/// in `pyproject.toml`.
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

/// Describes the role of a crate being added to the source distribution.
enum CrateRole<'a> {
    /// The main Python binding crate: rewrite workspace deps in Cargo.toml,
    /// skip pyproject.toml from `cargo package --list` (handled separately).
    Root {
        known_path_deps: &'a HashMap<String, PathDependency>,
        /// Path prefixes (relative to the manifest directory) whose files should be
        /// skipped from `cargo package --list` because they are added separately
        /// (e.g. python source directories that the explicit python source loop handles).
        /// See <https://github.com/PyO3/maturin/issues/2383>.
        skip_prefixes: Vec<PathBuf>,
    },
    /// A path dependency. When `skip_cargo_toml` is true, the crate's Cargo.toml
    /// is the workspace manifest and will be added separately with workspace-level rewrites.
    PathDependency { skip_cargo_toml: bool },
}

/// Copies the files of a crate to a source distribution, recursively adding path dependencies
/// and rewriting path entries in Cargo.toml
///
/// Runs `cargo package --list --allow-dirty` to obtain a list of files to package.
fn add_crate_to_source_distribution(
    writer: &mut VirtualWriter<SDistWriter>,
    manifest_path: impl AsRef<Path>,
    prefix: impl AsRef<Path>,
    readme: Option<&Path>,
    license_file: Option<&Path>,
    role: CrateRole<'_>,
) -> Result<()> {
    debug!(
        "Getting cargo package file list for {}",
        manifest_path.as_ref().display()
    );
    let prefix = prefix.as_ref();
    let manifest_path = manifest_path.as_ref();
    let args = ["package", "--list", "--allow-dirty", "--manifest-path"];
    let output = Command::new("cargo")
        .args(args)
        .arg(manifest_path)
        .output()
        .with_context(|| {
            format!(
                "Failed to run `cargo package --list --allow-dirty --manifest-path {}`",
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

    let file_list: Vec<&str> = str::from_utf8(&output.stdout)
        .context("Cargo printed invalid utf-8 ಠ_ಠ")?
        .lines()
        .collect();

    trace!("File list: {}", file_list.join(", "));

    // manifest_dir should be a relative path
    let manifest_dir = manifest_path.parent().unwrap();
    let skip_prefixes = match &role {
        CrateRole::Root { skip_prefixes, .. } => skip_prefixes.as_slice(),
        CrateRole::PathDependency { .. } => &[],
    };
    let target_source: Vec<_> = file_list
        .into_iter()
        .map(|relative_to_manifests| {
            let relative_to_cwd = manifest_dir.join(relative_to_manifests);
            (relative_to_manifests, relative_to_cwd)
        })
        .filter(|(target, source)| {
            if *target == "Cargo.toml.orig" {
                // Skip generated files. See https://github.com/rust-lang/cargo/issues/7938#issuecomment-593280660
                // and https://github.com/PyO3/maturin/issues/449
                false
            } else if *target == "Cargo.toml" {
                // We rewrite Cargo.toml and add it separately
                false
            } else if matches!(role, CrateRole::Root { .. }) && *target == "pyproject.toml" {
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
            } else if !skip_prefixes.is_empty()
                && skip_prefixes
                    .iter()
                    .any(|p| Path::new(target).starts_with(p))
            {
                // Skip files that will be added separately (e.g. python source files
                // that are added by the explicit python source loop).
                // This avoids duplicating them under the crate subdirectory when the
                // crate is a workspace member.
                // See https://github.com/PyO3/maturin/issues/2383
                debug!(
                    "Skipping {} (will be added separately)",
                    prefix.join(target).display()
                );
                false
            } else if is_compiled_artifact(Path::new(target)) {
                // Technically, `cargo package --list` should handle this,
                // but somehow it doesn't on Alpine Linux running in GitHub Actions,
                // so we do it manually here.
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

    let cargo_toml_path = prefix.join(manifest_path.file_name().unwrap());

    let readme_name = readme
        .as_ref()
        .map(|readme| {
            readme
                .file_name()
                .and_then(OsStr::to_str)
                .with_context(|| format!("Missing readme filename for {}", manifest_path.display()))
        })
        .transpose()?;

    let license_file_name = license_file
        .as_ref()
        .map(|lf| {
            lf.file_name().and_then(OsStr::to_str).with_context(|| {
                format!(
                    "Missing license-file filename for {}",
                    manifest_path.display()
                )
            })
        })
        .transpose()?;

    // Filter out files that were already added by resolve_and_add_readme /
    // resolve_and_add_file (e.g. readme or license-file from Cargo.toml pointing
    // outside the crate). `cargo package --list` may include a local copy at the
    // same target path, causing a duplicate.
    // See https://github.com/PyO3/maturin/issues/2358
    let target_source: Vec<_> = target_source
        .into_iter()
        .filter(|(target, _)| !writer.contains_target(prefix.join(target)))
        .collect();

    if !matches!(
        role,
        CrateRole::PathDependency {
            skip_cargo_toml: true
        }
    ) {
        let mut document = parse_toml_file(manifest_path, "Cargo.toml")?;
        rewrite_cargo_toml_readme(&mut document, manifest_path, readme_name)?;
        rewrite_cargo_toml_license_file(&mut document, manifest_path, license_file_name)?;
        if let CrateRole::Root {
            known_path_deps, ..
        } = &role
        {
            rewrite_cargo_toml(&mut document, manifest_path, known_path_deps)?;
        }
        rewrite_cargo_toml_targets(&mut document, manifest_path, &packaged_files)?;
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

/// Finds all path dependencies of the crate
pub fn find_path_deps(cargo_metadata: &Metadata) -> Result<HashMap<String, PathDependency>> {
    let root = cargo_metadata
        .root_package()
        .context("Expected the dependency graph to have a root package")?;

    // Pre-build lookup indices to avoid repeated linear scans
    let packages_by_id: HashMap<&PackageId, &cargo_metadata::Package> =
        cargo_metadata.packages.iter().map(|p| (&p.id, p)).collect();
    let pkg_readmes: HashMap<&PackageId, PathBuf> = cargo_metadata
        .packages
        .iter()
        .filter_map(|package| {
            package
                .readme
                .as_ref()
                .map(|readme| (&package.id, readme.clone().into_std_path_buf()))
        })
        .collect();
    let pkg_license_files: HashMap<&PackageId, PathBuf> = cargo_metadata
        .packages
        .iter()
        .filter_map(|package| {
            package
                .license_file
                .as_ref()
                .map(|license_file| (&package.id, license_file.clone().into_std_path_buf()))
        })
        .collect();
    let resolve_nodes: HashMap<&PackageId, &[cargo_metadata::NodeDep]> = cargo_metadata
        .resolve
        .as_ref()
        .context("cargo metadata is missing dependency resolve information")?
        .nodes
        .iter()
        .map(|node| (&node.id, node.deps.as_slice()))
        .collect();

    // Scan the dependency graph for path dependencies
    let mut path_deps = HashMap::new();
    let mut stack: Vec<&cargo_metadata::Package> = vec![root];
    while let Some(top) = stack.pop() {
        let node_deps = resolve_nodes
            .get(&top.id)
            .with_context(|| format!("missing resolve node for package {}", top.id))?;
        for node_dep in *node_deps {
            let dep_pkg = packages_by_id
                .get(&node_dep.pkg)
                .with_context(|| format!("missing package metadata for {}", node_dep.pkg))?;
            // Match the resolved dependency back to the declared dependency
            // to check if it's a path dependency.
            let dependency = top
                .dependencies
                .iter()
                .find(|d| d.name == dep_pkg.name.as_ref())
                .with_context(|| {
                    format!(
                        "could not find dependency {} in package {}",
                        dep_pkg.name, top.id
                    )
                })?;
            if let Some(path) = &dependency.path {
                let dep_name = dependency.rename.as_ref().unwrap_or(&dependency.name);
                if path_deps.contains_key(dep_name) {
                    continue;
                }
                let dep_manifest_path = path.join("Cargo.toml");
                // Path dependencies may not be in the same workspace as the root crate,
                // thus we need to find out its workspace root from `cargo metadata`
                let path_dep_metadata = MetadataCommand::new()
                    .manifest_path(&dep_manifest_path)
                    .verbose(true)
                    // We don't need to resolve the dependency graph
                    .no_deps()
                    .exec()
                    .with_context(|| {
                        format!(
                            "Failed to resolve workspace root for {} at '{dep_manifest_path}'",
                            node_dep.pkg
                        )
                    })?;

                path_deps.insert(
                    dep_name.clone(),
                    PathDependency {
                        manifest_path: PathBuf::from(dep_manifest_path.clone()),
                        workspace_root: path_dep_metadata
                            .workspace_root
                            .clone()
                            .into_std_path_buf(),
                        readme: pkg_readmes.get(&node_dep.pkg).cloned(),
                        license_file: pkg_license_files.get(&node_dep.pkg).cloned(),
                    },
                );
                // Continue scanning the path dependency's own dependencies
                if let Some(&dep_package) = packages_by_id.get(&node_dep.pkg) {
                    stack.push(dep_package)
                }
            }
        }
    }
    Ok(path_deps)
}

/// Copies the files of git to a source distribution
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

/// Copies the files of a crate to a source distribution, recursively adding path dependencies
/// and rewriting path entries in Cargo.toml
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
    let python_dir = &build_context.project_layout.python_dir;
    if !python_dir.starts_with(&sdist_root)
        && let Some(prefix) = common_path_prefix(&sdist_root, python_dir)
    {
        sdist_root = prefix;
    }

    debug!("Found sdist root: {}", sdist_root.display());

    let ctx = SdistContext {
        root_dir,
        workspace_root,
        workspace_manifest_path,
        known_path_deps,
        sdist_root,
    };

    // Add local path dependencies
    for (name, path_dep) in ctx.known_path_deps.iter() {
        add_path_dep(writer, &ctx, name, path_dep)
            .with_context(|| format!("Failed to add path dependency {name}"))?;
    }

    debug!("Adding the main crate {}", manifest_path.display());
    // Add the main crate
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
    // Handle possible relative readme field in Cargo.toml
    let readme_path = if let Some(readme) = main_crate.readme.as_ref() {
        let target_dir = root_dir.join(relative_main_crate_manifest_dir);
        Some(resolve_and_add_readme(
            writer,
            readme.as_std_path(),
            abs_manifest_dir,
            &target_dir,
        )?)
    } else {
        None
    };
    // Handle possible relative license-file field in Cargo.toml
    let license_file_path = if let Some(license_file) = main_crate.license_file.as_ref() {
        let target_dir = root_dir.join(relative_main_crate_manifest_dir);
        Some(resolve_and_add_file(
            writer,
            license_file.as_std_path(),
            abs_manifest_dir,
            &target_dir,
            "license-file",
            Some(workspace_root.as_std_path()),
        )?)
    } else {
        None
    };
    // Compute python source directories relative to the manifest directory.
    // When the crate is a workspace member in a subdirectory, `cargo package --list`
    // includes python source files that will also be added by the explicit python
    // source loop (relative to pyproject_dir). We skip them here to avoid duplicates
    // in the sdist. See https://github.com/PyO3/maturin/issues/2383
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
        root_dir.join(relative_main_crate_manifest_dir),
        readme_path.as_deref(),
        license_file_path.as_deref(),
        CrateRole::Root {
            known_path_deps: &ctx.known_path_deps,
            skip_prefixes,
        },
    )?;

    // Add Cargo.lock file
    let manifest_cargo_lock_path = abs_manifest_dir.join("Cargo.lock");
    let workspace_cargo_lock = ctx.workspace_root.join("Cargo.lock").into_std_path_buf();
    let cargo_lock_path = if manifest_cargo_lock_path.exists() {
        Some(manifest_cargo_lock_path.clone())
    } else if workspace_cargo_lock.exists() {
        Some(workspace_cargo_lock)
    } else {
        None
    };
    let cargo_lock_required =
        build_context.cargo_options.locked || build_context.cargo_options.frozen;
    // Determine the project root for computing relative paths inside the sdist.
    // This is the outermost directory that contains both pyproject.toml and the
    // sdist root (which accounts for workspace root and all path dependencies).
    let pyproject_root = pyproject_toml_path.parent().unwrap();
    let project_root =
        if pyproject_root == ctx.sdist_root || pyproject_root.starts_with(&ctx.sdist_root) {
            &ctx.sdist_root
        } else {
            assert!(ctx.sdist_root.starts_with(pyproject_root));
            pyproject_root
        };
    if let Some(cargo_lock_path) = cargo_lock_path {
        let relative_cargo_lock = cargo_lock_path.strip_prefix(project_root).unwrap();
        writer.add_file(root_dir.join(relative_cargo_lock), &cargo_lock_path, false)?;
    } else if cargo_lock_required {
        bail!("Cargo.lock is required by `--locked`/`--frozen` but it's not found.");
    } else {
        eprintln!(
            "⚠️  Warning: Cargo.lock is not found, it is recommended \
            to include it in the source distribution"
        );
    }

    // Add workspace Cargo.toml when the crate is a workspace member.
    // Without it, cargo can't resolve workspace-level dependencies from the sdist.
    // Note: when a crate is `exclude`d from a workspace, `cargo metadata` reports
    // the crate's own directory as `workspace_root`, so this check correctly
    // skips adding the parent workspace Cargo.toml for excluded crates.
    //
    // We normalize workspace_root to match abs_manifest_dir (also normalized) so
    // that symlinks or .. components don't cause a false positive. A false positive
    // here would try to add a rewritten workspace Cargo.toml on top of the main
    // crate's Cargo.toml, causing a duplicate-file error in VirtualWriter.
    let normalized_workspace_root = ctx
        .workspace_root
        .as_std_path()
        .normalize()
        .map(|p| p.into_path_buf())
        .unwrap_or_else(|_| ctx.workspace_root.as_std_path().to_path_buf());
    let is_in_workspace = normalized_workspace_root != abs_manifest_dir;
    if is_in_workspace {
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
                manifest_path: manifest_path.clone(),
                workspace_root: ctx.workspace_root.as_std_path().to_path_buf(),
                readme: None,
                license_file: None,
            },
        );
        // Rewrite workspace Cargo.toml to only include relevant members,
        // removing unrelated workspace members to keep the sdist minimal.
        let mut document =
            parse_toml_file(ctx.workspace_manifest_path.as_std_path(), "Cargo.toml")?;
        rewrite_cargo_toml(
            &mut document,
            ctx.workspace_manifest_path.as_std_path(),
            &deps_to_keep,
        )?;
        // When the workspace root Cargo.toml is also a [package] (virtual
        // workspaces don't have one), the package's source files are typically
        // not included in the sdist. Cargo will fail to parse the manifest if
        // it can't find the library/binary source. Strip the [package] section
        // and all package-level tables so cargo treats it as a virtual workspace.
        let workspace_root_is_path_dep = ctx
            .known_path_deps
            .values()
            .any(|dep| dep.manifest_path.as_path() == ctx.workspace_manifest_path.as_std_path());
        if !workspace_root_is_path_dep && document.contains_key("package") {
            debug!(
                "Stripping [package] from workspace Cargo.toml at {} (source files not in sdist)",
                ctx.workspace_manifest_path
            );
            // Remove all sections that belong to the [package] crate, keeping
            // only workspace-level tables ([workspace], [workspace.*], [profile]).
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
            root_dir.join(relative_workspace_cargo_toml),
            Some(ctx.workspace_manifest_path.as_std_path()),
            document.to_string().as_bytes(),
            false,
        )?;
    }

    // Add pyproject.toml
    let pyproject_dir = pyproject_toml_path.parent().unwrap();
    if pyproject_dir != ctx.sdist_root {
        // rewrite `tool.maturin.manifest-path` and `tool.maturin.python-source` in pyproject.toml
        let python_dir = &build_context.project_layout.python_dir;
        // Compute python-source relative to pyproject_dir.  When python_dir is
        // outside pyproject_dir (e.g. `python-source = "../../python"`), compute
        // the path relative to project_root instead (the sdist root) so that the
        // rewritten pyproject.toml can still find the files.
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
            root_dir.join("pyproject.toml"),
            Some(pyproject_toml_path),
            rewritten_pyproject_toml.as_bytes(),
            false,
        )?;
    } else {
        writer.add_file(root_dir.join("pyproject.toml"), pyproject_toml_path, false)?;
    }

    // Add python source files
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
            // Technically, `ignore` crate should handle this,
            // but somehow it doesn't on Alpine Linux running in GitHub Actions,
            // so we do it manually here.
            // See https://github.com/PyO3/maturin/pull/1187#issuecomment-1273987013
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
            let target = root_dir.join(relative);
            if !source.is_dir() {
                writer.add_file(target, &source, false)?;
            }
        }
    }

    Ok(())
}

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

    // Handle possible relative readme field in Cargo.toml
    let readme_path = if let Some(readme) = path_dep.readme.as_ref() {
        let target_dir = ctx.root_dir.join(relative_path_dep_manifest_dir);
        Some(resolve_and_add_readme(
            writer,
            readme,
            path_dep_manifest_dir,
            &target_dir,
        )?)
    } else {
        None
    };

    // Handle possible relative license-file field in Cargo.toml
    let license_file_path = if let Some(license_file) = path_dep.license_file.as_ref() {
        let target_dir = ctx.root_dir.join(relative_path_dep_manifest_dir);
        Some(resolve_and_add_file(
            writer,
            license_file,
            path_dep_manifest_dir,
            &target_dir,
            "license-file",
            Some(&path_dep.workspace_root),
        )?)
    } else {
        None
    };

    add_crate_to_source_distribution(
        writer,
        &path_dep.manifest_path,
        ctx.root_dir.join(relative_path_dep_manifest_dir),
        readme_path.as_deref(),
        license_file_path.as_deref(),
        CrateRole::PathDependency { skip_cargo_toml },
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
        let relative_path_dep_workspace_manifest = path_dep_workspace_manifest
            .strip_prefix(&ctx.sdist_root)
            .unwrap();
        writer.add_file(
            ctx.root_dir.join(relative_path_dep_workspace_manifest),
            &path_dep_workspace_manifest,
            false,
        )?;
    }
    Ok(())
}

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
    // Add readme, license
    // Skip if the target path was already added (e.g. from Cargo.toml metadata)
    // to avoid "was already added" errors when both Cargo.toml and pyproject.toml
    // specify a readme/license pointing to different files.
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
            // Safe on Windows and Unix as neither forward nor backwards slashes are escaped.
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

/// Find the common prefix, if any, between two paths
///
/// Taken from https://docs.rs/common-path/1.0.0/src/common_path/lib.rs.html#84-109
/// License: MIT/Apache 2.0
fn common_path_prefix<P, Q>(one: P, two: Q) -> Option<PathBuf>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    let one = one.as_ref();
    let two = two.as_ref();
    let one = one.components();
    let two = two.components();
    let mut final_path = PathBuf::new();
    let mut found = false;
    let paths = one.zip(two);
    for (l, r) in paths {
        if l == r {
            final_path.push(l.as_os_str());
            found = true;
        } else {
            break;
        }
    }
    if found { Some(final_path) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo_metadata::MetadataCommand;
    use fs_err as fs;
    use ignore::overrides::Override;
    use pep440_rs::Version;
    use std::str::FromStr;
    use tempfile::TempDir;

    use crate::Metadata24;

    #[test]
    fn test_normalize_path() {
        let test_cases = vec![
            // Basic paths without normalization needed
            ("foo/bar", "foo/bar"),
            ("src/lib.rs", "src/lib.rs"),
            // Remove current directory components
            ("./foo/bar", "foo/bar"),
            ("foo/./bar", "foo/bar"),
            (".", ""),
            // Parent directory that cancels with a normal component
            ("foo/../bar", "bar"),
            ("foo/bar/..", "foo"),
            ("foo/bar/../baz", "foo/baz"),
            // Leading parent directories should be preserved
            ("../foo", "../foo"),
            ("../../foo", "../../foo"),
            ("../../../foo/bar", "../../../foo/bar"),
            // More parent dirs than normal components
            ("a/../../b", "../b"),
            ("a/b/../../../c", "../c"),
            ("foo/bar/baz/../../..", ""),
            // Mix of current and parent directory components
            ("./foo/../bar", "bar"),
            ("foo/./bar/../baz", "foo/baz"),
            ("./../foo/./bar", "../foo/bar"),
            // Edge cases
            ("", ""),
            ("foo", "foo"),
            ("..", ".."),
        ];

        for (input, expected) in test_cases {
            assert_eq!(
                normalize_path(Path::new(input)),
                PathBuf::from(expected),
                "normalize_path({:?}) should equal {:?}",
                input,
                expected
            );
        }
    }

    #[test]
    fn test_copy_manifest_sidecar_file_rejects_license_outside_allowed_root() {
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

        let err = resolve_and_add_file(
            &mut writer,
            Path::new("../../SECRET_LICENSE"),
            &manifest_dir,
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

    #[test]
    fn test_find_path_deps_captures_workspace_license_file() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_root = temp_dir.path();
        let py_dir = workspace_root.join("py");
        let dep_dir = workspace_root.join("dep");

        fs::create_dir_all(py_dir.join("src")).unwrap();
        fs::create_dir_all(dep_dir.join("src")).unwrap();
        fs::write(py_dir.join("src/lib.rs"), "").unwrap();
        fs::write(dep_dir.join("src/lib.rs"), "").unwrap();
        fs::write(workspace_root.join("LICENSE"), "MIT").unwrap();

        fs::write(
            workspace_root.join("Cargo.toml"),
            indoc::indoc!(
                r#"
                [workspace]
                resolver = "2"
                members = ["py", "dep"]

                [workspace.package]
                license-file = "LICENSE"
                "#
            ),
        )
        .unwrap();

        fs::write(
            dep_dir.join("Cargo.toml"),
            indoc::indoc!(
                r#"
                [package]
                name = "dep"
                version = "0.1.0"
                edition = "2021"
                license-file.workspace = true
                "#
            ),
        )
        .unwrap();

        fs::write(
            py_dir.join("Cargo.toml"),
            indoc::indoc!(
                r#"
                [package]
                name = "py"
                version = "0.1.0"
                edition = "2021"

                [dependencies]
                dep = { path = "../dep" }
                "#
            ),
        )
        .unwrap();

        let cargo_metadata = MetadataCommand::new()
            .manifest_path(py_dir.join("Cargo.toml"))
            .exec()
            .unwrap();

        let path_deps = find_path_deps(&cargo_metadata).unwrap();
        let dep = path_deps.get("dep").expect("missing path dependency");
        assert_eq!(dep.license_file.as_deref(), Some(Path::new("../LICENSE")));
    }

    #[test]
    fn test_rewrite_cargo_toml_license_file() {
        let manifest_path = Path::new("Cargo.toml");

        // When license_file_name is Some, it should rewrite the field
        let toml_str = r#"
[package]
name = "test"
version = "0.1.0"
license-file = "../../LICENSE"
"#;
        let mut document = toml_str.parse::<DocumentMut>().unwrap();
        rewrite_cargo_toml_license_file(&mut document, manifest_path, Some("LICENSE")).unwrap();
        let result = document.to_string();
        assert!(
            result.contains(r#"license-file = "LICENSE""#),
            "expected rewritten license-file, got: {result}"
        );

        // When license_file_name is None, it should be a no-op
        let mut document2 = toml_str.parse::<DocumentMut>().unwrap();
        rewrite_cargo_toml_license_file(&mut document2, manifest_path, None).unwrap();
        let result2 = document2.to_string();
        assert!(
            result2.contains(r#"license-file = "../../LICENSE""#),
            "expected unchanged license-file, got: {result2}"
        );
    }

    #[test]
    fn test_rewrite_cargo_toml_removes_default_members() {
        let manifest_path = Path::new("Cargo.toml");
        let toml_str = r#"
[workspace]
members = ["crate-a", "crate-b"]
default-members = ["crate-a", "crate-c"]
"#;
        let mut document = toml_str.parse::<DocumentMut>().unwrap();
        let mut known_path_deps = HashMap::new();
        known_path_deps.insert(
            "crate-a".to_string(),
            PathDependency {
                manifest_path: PathBuf::from("crate-a/Cargo.toml"),
                workspace_root: PathBuf::from(""),
                readme: None,
                license_file: None,
            },
        );
        rewrite_cargo_toml(&mut document, manifest_path, &known_path_deps).unwrap();
        let result = document.to_string();
        assert!(
            result.contains(r#"members = ["crate-a"]"#),
            "expected filtered members, got: {result}"
        );
        assert!(
            !result.contains("default-members"),
            "expected default-members to be removed, got: {result}"
        );
    }
}
