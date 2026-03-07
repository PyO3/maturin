use std::path::{Component, Path, PathBuf};

/// Returns `true` if the file extension indicates a compiled artifact
/// that should be excluded from the source distribution.
pub(super) fn is_compiled_artifact(path: &Path) -> bool {
    matches!(path.extension(), Some(ext) if ext == "pyc" || ext == "pyd" || ext == "so")
}

/// Normalize a path by resolving `.` and `..` components without filesystem access.
pub(super) fn normalize_path(path: &Path) -> PathBuf {
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

/// Find the common prefix, if any, between two paths.
///
/// Taken from https://docs.rs/common-path/1.0.0/src/common_path/lib.rs.html#84-109
/// License: MIT/Apache 2.0
pub(super) fn common_path_prefix<P, Q>(one: P, two: Q) -> Option<PathBuf>
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

/// Compute a relative path from `from` directory to `to` path.
///
/// Both absolute and relative inputs are supported. Returns a relative path
/// that, when joined with `from`, resolves to `to`. If the paths share no
/// common prefix (for example, different Windows drive letters or disjoint
/// top-level components), the original `to` path is returned unchanged since
/// no relative traversal is possible.
pub(super) fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let Some(common) = common_path_prefix(from, to) else {
        // No common prefix — fall back to the target path as-is.
        return to.to_path_buf();
    };
    let from_rest = from.strip_prefix(&common).unwrap_or(Path::new(""));
    let to_rest = to.strip_prefix(&common).unwrap_or(Path::new(""));
    let mut result = PathBuf::new();
    for _ in from_rest.components() {
        result.push("..");
    }
    result.push(to_rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_relative_path() {
        let test_cases = vec![
            ("/a/b/c", "/a/b/d", "../d"),
            ("/a/b", "/a/b/c/d", "c/d"),
            ("/a/b/c", "/a/b", ".."),
            ("/a/b/c", "/a/x/y", "../../x/y"),
            ("/a/b", "/a/b", ""),
            // No common prefix — returns the absolute target as-is
            ("foo", "/x/y", "/x/y"),
        ];
        for (from, to, expected) in test_cases {
            assert_eq!(
                relative_path(Path::new(from), Path::new(to)),
                PathBuf::from(expected),
                "relative_path({from:?}, {to:?}) should equal {expected:?}",
            );
        }
    }
}
