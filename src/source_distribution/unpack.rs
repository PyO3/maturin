use crate::PyProjectToml;
use anyhow::{Context, Result, bail};
use fs_err as fs;
use std::path::{Path, PathBuf};

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
