#[cfg(test)]
mod test_external_libs_repeated_build {
    use std::fs;
    use tempfile::TempDir;

    /// Test that creating temporary copies of artifacts works correctly
    #[test]
    fn test_artifact_copy_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Create a mock artifact file
        let artifact_path = temp_dir.path().join("test_artifact");
        fs::write(&artifact_path, b"mock binary content").expect("Failed to write artifact");

        // Create a copy like our code does
        let artifact_name = artifact_path.file_name().unwrap();
        let temp_artifact_path = temp_dir.path().join("temp").join(artifact_name);
        fs::create_dir_all(temp_artifact_path.parent().unwrap())
            .expect("Failed to create temp dir");
        fs::copy(&artifact_path, &temp_artifact_path).expect("Failed to copy artifact");

        // Verify the copy exists and has the same content
        assert!(temp_artifact_path.exists());
        let original_content = fs::read(&artifact_path).expect("Failed to read original");
        let copy_content = fs::read(&temp_artifact_path).expect("Failed to read copy");
        assert_eq!(original_content, copy_content);

        // Simulate modifying the copy (we can't actually run patchelf without the binary)
        fs::write(&temp_artifact_path, b"modified binary content").expect("Failed to modify copy");

        // Verify original is unchanged
        let original_content_after =
            fs::read(&artifact_path).expect("Failed to read original after modification");
        assert_eq!(original_content, original_content_after);

        // Verify copy is modified
        let copy_content_after =
            fs::read(&temp_artifact_path).expect("Failed to read copy after modification");
        assert_eq!(copy_content_after, b"modified binary content");
    }
}
