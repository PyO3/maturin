use crate::pyproject_toml::Format;
use anyhow::{Context, Result, bail};
use std::path::{Component, Path, PathBuf};

/// A resolved include match: the source file on disk and the relative path
/// it should have inside the archive (wheel or sdist).
pub(crate) struct IncludeMatch {
    /// Absolute path to the file on disk.
    pub source: PathBuf,
    /// Relative path inside the archive.
    pub target: PathBuf,
}

/// Resolve include glob patterns, trying `project_root` first and falling back
/// to `python_dir` when `python-source` is set and the pattern didn't match.
///
/// The `format` determines how matched files are mapped to archive paths:
/// - **Wheel**: files inside `python_dir` are stripped to `python_dir`-relative
///   paths (so `src/python/pkg/data.txt` becomes `pkg/data.txt`); other files
///   are stripped to `project_root`-relative paths.
/// - **Sdist**: files are always relative to `project_root`. When matched via
///   the `python_dir` fallback, they are re-rooted under the `python-source`
///   prefix to preserve the source layout.
pub(crate) fn resolve_include_matches(
    pattern: &str,
    format: Format,
    project_root: &Path,
    python_dir: &Path,
) -> Result<Vec<IncludeMatch>> {
    validate_pattern(pattern)?;

    let mut matches = glob_files(project_root, pattern)?;

    // When python-source is set and the pattern didn't match any files
    // relative to project_root, also try relative to python_dir so that a
    // single pattern like "pyfoo/data.txt" works for both sdist and wheel.
    if matches.is_empty() && python_dir != project_root {
        matches = glob_files(python_dir, pattern)?;
    }

    // Map source paths to archive-relative target paths.
    let matches = matches
        .into_iter()
        .map(|(source, matched_root)| {
            let target = map_target(format, &source, matched_root, project_root, python_dir)?;
            Ok(IncludeMatch { source, target })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(matches)
}

/// Map a matched source file to its archive-relative target path.
fn map_target(
    format: Format,
    source: &Path,
    matched_root: &Path,
    project_root: &Path,
    python_dir: &Path,
) -> Result<PathBuf> {
    match format {
        Format::Wheel => {
            // For wheel: always prefer stripping python_dir for files inside it,
            // so "src/python/pkg/data.txt" becomes "pkg/data.txt".
            if python_dir != project_root && source.starts_with(python_dir) {
                Ok(source.strip_prefix(python_dir)?.to_path_buf())
            } else {
                Ok(source.strip_prefix(matched_root)?.to_path_buf())
            }
        }
        Format::Sdist => {
            let relative = source.strip_prefix(matched_root)?;
            if matched_root == python_dir && python_dir != project_root {
                // Re-root under the python-source prefix so the sdist
                // preserves the original directory layout.
                //
                // `python_dir` may be outside `project_root` (e.g. when
                // `python-source = "../../python"`), so we can't assume
                // it is always a subdirectory. Prefer project-root-relative
                // prefix when possible, but fall back to the final component.
                let py_prefix = match python_dir.strip_prefix(project_root) {
                    Ok(rel) => rel.to_path_buf(),
                    Err(_) => {
                        let basename = python_dir.file_name().with_context(|| {
                            format!(
                                "python-source `{}` has no final path component",
                                python_dir.display()
                            )
                        })?;
                        PathBuf::from(basename)
                    }
                };
                Ok(py_prefix.join(relative))
            } else {
                Ok(relative.to_path_buf())
            }
        }
    }
}

/// Glob for files under `root` matching `pattern`, skipping directories.
/// Returns `(source_path, matched_root)` pairs.
fn glob_files<'a>(root: &'a Path, pattern: &str) -> Result<Vec<(PathBuf, &'a Path)>> {
    let escaped_root = PathBuf::from(glob::Pattern::escape(root.to_string_lossy().as_ref()));
    let full_pattern = escaped_root.join(pattern);

    let mut matches = Vec::new();
    for source in glob::glob(&full_pattern.to_string_lossy())
        .with_context(|| format!("Invalid glob pattern: {pattern}"))?
        .filter_map(Result::ok)
    {
        if source.is_dir() {
            continue;
        }
        matches.push((source, root));
    }
    Ok(matches)
}

/// Resolve out-dir include patterns: globs files matching `pattern` under
/// `out_dir` and maps each match to `{to}/{relative_path}` inside the wheel.
pub(crate) fn resolve_out_dir_includes(
    pattern: &str,
    out_dir: &Path,
    to: &str,
) -> Result<Vec<IncludeMatch>> {
    validate_pattern(pattern)?;
    validate_pattern(to)?;

    let matches = glob_files(out_dir, pattern)?;
    matches
        .into_iter()
        .map(|(source, matched_root)| {
            let relative = source.strip_prefix(matched_root)?;
            let target = PathBuf::from(to).join(relative);
            Ok(IncludeMatch { source, target })
        })
        .collect()
}

/// Reject patterns that are not purely relative (absolute, contain `..`, or
/// have a Windows drive/UNC prefix).
fn validate_pattern(pattern: &str) -> Result<()> {
    for component in Path::new(pattern).components() {
        match component {
            Component::Normal(_) => {}
            Component::ParentDir => {
                bail!(
                    "include/exclude pattern must not contain `..`, got: {pattern}. \
                     Use a pattern relative to the project root or python-source directory."
                );
            }
            _ => {
                // Rejects Component::RootDir, Component::Prefix (Windows drive/UNC),
                // and Component::CurDir (`.` is harmless but noisy).
                bail!("include/exclude pattern must be a relative path, got: {pattern}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err as fs;
    use tempfile::TempDir;

    #[test]
    fn test_validate_pattern() {
        // Rejected: absolute, parent dir, current dir
        assert!(validate_pattern("/foo/bar").is_err());
        assert!(validate_pattern("../foo/bar").is_err());
        assert!(validate_pattern("foo/../bar").is_err());
        assert!(validate_pattern("./foo/bar").is_err());

        // Allowed: normal relative patterns
        assert!(validate_pattern("foo/bar").is_ok());
        assert!(validate_pattern("**/*.html").is_ok());
        assert!(validate_pattern("pyfoo/bar.html").is_ok());
    }

    /// Helper to create a temp dir with files at the given relative paths.
    fn setup_tree(files: &[&str]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for file in files {
            let path = dir.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "content").unwrap();
        }
        dir
    }

    #[test]
    fn test_resolve_matches_from_primary_root() {
        let dir = setup_tree(&["pkg/data.txt"]);
        let root = dir.path();

        let matches = resolve_include_matches("pkg/data.txt", Format::Wheel, root, root).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].target, Path::new("pkg/data.txt"));
    }

    #[test]
    fn test_resolve_python_dir_fallback() {
        let dir = setup_tree(&["src/python/pkg/data.txt"]);
        let root = dir.path();
        let python_dir = root.join("src/python");

        // Wheel: target is python_dir-relative
        let wheel =
            resolve_include_matches("pkg/data.txt", Format::Wheel, root, &python_dir).unwrap();
        assert_eq!(wheel.len(), 1);
        assert_eq!(wheel[0].target, Path::new("pkg/data.txt"));
        assert_eq!(wheel[0].source, python_dir.join("pkg/data.txt"));

        // Sdist: target preserves the python-source prefix
        let sdist =
            resolve_include_matches("pkg/data.txt", Format::Sdist, root, &python_dir).unwrap();
        assert_eq!(sdist.len(), 1);
        assert_eq!(sdist[0].target, Path::new("src/python/pkg/data.txt"));
    }

    #[test]
    fn test_wheel_strips_python_dir_for_explicit_path() {
        let dir = setup_tree(&["src/python/pkg/data.txt"]);
        let root = dir.path();
        let python_dir = root.join("src/python");

        let matches =
            resolve_include_matches("src/python/pkg/data.txt", Format::Wheel, root, &python_dir)
                .unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].target, Path::new("pkg/data.txt"));
    }

    #[test]
    fn test_primary_root_takes_precedence() {
        let dir = setup_tree(&["pkg/data.txt", "src/python/pkg/data.txt"]);
        let root = dir.path();
        let python_dir = root.join("src/python");

        let matches =
            resolve_include_matches("pkg/data.txt", Format::Wheel, root, &python_dir).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].source, root.join("pkg/data.txt"));
    }

    #[test]
    fn test_python_dir_outside_project_root() {
        // Simulate python-source pointing outside the project root
        let python_tmp = setup_tree(&["pkg/data.txt"]);
        let project_tmp = TempDir::new().unwrap();
        let python_dir = python_tmp.path();
        let project_root = project_tmp.path();

        // Sdist: should use the final component of python_dir as prefix
        let sdist =
            resolve_include_matches("pkg/data.txt", Format::Sdist, project_root, python_dir)
                .unwrap();
        assert_eq!(sdist.len(), 1);
        let python_dir_name = python_dir.file_name().unwrap();
        assert_eq!(
            sdist[0].target,
            Path::new(python_dir_name).join("pkg/data.txt")
        );

        // Wheel: should strip python_dir prefix
        let wheel =
            resolve_include_matches("pkg/data.txt", Format::Wheel, project_root, python_dir)
                .unwrap();
        assert_eq!(wheel.len(), 1);
        assert_eq!(wheel[0].target, Path::new("pkg/data.txt"));
    }

    #[test]
    fn test_out_dir_validates_to_parameter() {
        let dir = setup_tree(&["gen.txt"]);
        assert!(resolve_out_dir_includes("gen.txt", dir.path(), "../escape").is_err());
        assert!(resolve_out_dir_includes("gen.txt", dir.path(), "/absolute").is_err());
        assert!(resolve_out_dir_includes("gen.txt", dir.path(), "pkg/").is_ok());
    }

    #[test]
    fn test_no_match_and_dir_only() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("pkg/subdir")).unwrap();

        // No file matches
        assert!(
            resolve_include_matches("nonexistent.txt", Format::Wheel, root, root)
                .unwrap()
                .is_empty()
        );

        // Glob matching only a directory returns no results
        assert!(
            resolve_include_matches("pkg/*", Format::Wheel, root, root)
                .unwrap()
                .is_empty()
        );
    }
}
