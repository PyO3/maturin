use std::path::{Path, PathBuf};

/// Resolve the path to a test crate.
pub fn test_crate_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test-crates")
        .join(name)
}
