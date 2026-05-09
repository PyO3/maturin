use crate::common::{handle_result, other};

#[test]
#[cfg(feature = "sbom")]
fn pyo3_pure_sbom_wheel_files() {
    handle_result(other::check_wheel_files_with_sbom(
        "test-crates/pyo3-pure",
        "wheel-files-pyo3-pure-sbom",
    ))
}

#[test]
fn pyo3_mixed_include_exclude_wheel_files() {
    #[allow(unused_mut)]
    let mut expected = vec![
        "pyo3_mixed_include_exclude-2.1.3.dist-info/METADATA",
        "pyo3_mixed_include_exclude-2.1.3.dist-info/RECORD",
        "pyo3_mixed_include_exclude-2.1.3.dist-info/WHEEL",
        "pyo3_mixed_include_exclude-2.1.3.dist-info/entry_points.txt",
        "pyo3_mixed_include_exclude/__init__.py",
        "pyo3_mixed_include_exclude/generated_info.txt",
        "pyo3_mixed_include_exclude/include_this_file",
        "pyo3_mixed_include_exclude/python_module/__init__.py",
        "pyo3_mixed_include_exclude/python_module/double.py",
        "README.md",
    ];
    #[cfg(feature = "sbom")]
    expected.push(
        "pyo3_mixed_include_exclude-2.1.3.dist-info/sboms/pyo3-mixed-include-exclude.cyclonedx.json",
    );
    handle_result(other::check_wheel_files(
        "test-crates/pyo3-mixed-include-exclude",
        expected,
        "wheel-files-pyo3-mixed-include-exclude",
    ))
}

#[test]
fn pyo3_mixed_py_subdir_include_wheel_files() {
    #[allow(unused_mut)]
    let mut expected = vec![
        "pyo3_mixed_py_subdir-2.1.3.dist-info/METADATA",
        "pyo3_mixed_py_subdir-2.1.3.dist-info/RECORD",
        "pyo3_mixed_py_subdir-2.1.3.dist-info/WHEEL",
        "pyo3_mixed_py_subdir-2.1.3.dist-info/entry_points.txt",
        "pyo3_mixed_py_subdir/__init__.py",
        "pyo3_mixed_py_subdir/python_module/__init__.py",
        "pyo3_mixed_py_subdir/python_module/double.py",
        "assets/extra_data.txt",
    ];
    #[cfg(feature = "sbom")]
    expected.push("pyo3_mixed_py_subdir-2.1.3.dist-info/sboms/pyo3-mixed-py-subdir.cyclonedx.json");
    handle_result(other::check_wheel_files(
        "test-crates/pyo3-mixed-py-subdir",
        expected,
        "wheel-files-pyo3-mixed-py-subdir-include",
    ))
}

#[test]
#[cfg(unix)]
fn pyo3_mixed_py_subdir_includes_symlinked_python_files() {
    handle_result((|| {
        let (_temp_dir, project_dir) = other::copy_pyo3_mixed_py_subdir_with_symlinks()?;

        let mut expected = vec![
            "assets/extra_data.txt",
            "pyo3_mixed_py_subdir-2.1.3.dist-info/METADATA",
            "pyo3_mixed_py_subdir-2.1.3.dist-info/RECORD",
            "pyo3_mixed_py_subdir-2.1.3.dist-info/WHEEL",
            "pyo3_mixed_py_subdir-2.1.3.dist-info/entry_points.txt",
            "pyo3_mixed_py_subdir/__init__.py",
            "pyo3_mixed_py_subdir/python_module/__init__.py",
            "pyo3_mixed_py_subdir/python_module/double.py",
            "pyo3_mixed_py_subdir/python_module/linked_dir/nested.py",
            "pyo3_mixed_py_subdir/python_module/linked_file.py",
        ];
        #[cfg(feature = "sbom")]
        expected
            .push("pyo3_mixed_py_subdir-2.1.3.dist-info/sboms/pyo3-mixed-py-subdir.cyclonedx.json");

        assert_eq!(
            other::wheel_files(&project_dir, "wheel-files-pyo3-mixed-py-subdir-symlinks")?,
            expected.into_iter().map(str::to_owned).collect()
        );

        Ok(())
    })())
}

#[test]
fn pyo3_wheel_record_has_normalized_paths() {
    handle_result(other::check_wheel_paths(
        "test-crates/pyo3-mixed-include-exclude",
        "pyo3_mixed_include_exclude-2.1.3.dist-info/RECORD",
        "wheel-record-pyo3-mixed-include-exclude",
    ))
}
