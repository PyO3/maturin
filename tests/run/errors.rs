use crate::common::{errors, handle_result, test_python_implementation};

#[test]
#[cfg_attr(
    not(all(
        target_os = "linux",
        target_env = "gnu",
        any(
            target_arch = "x86",
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "powerpc64",
            target_arch = "s390x",
            target_arch = "arm"
        )
    )),
    ignore
)]
fn pyo3_no_extension_module() {
    if test_python_implementation().unwrap() == "cpython" {
        handle_result(errors::pyo3_no_extension_module())
    }
}

#[test]
fn locked_doesnt_build_without_cargo_lock() {
    handle_result(errors::locked_doesnt_build_without_cargo_lock())
}

#[test]
#[cfg_attr(not(all(target_os = "linux", target_env = "gnu")), ignore)]
fn invalid_manylinux_does_not_panic() {
    handle_result(errors::invalid_manylinux_does_not_panic())
}

#[test]
fn error_on_missing_python_source() {
    handle_result(errors::error_on_missing_python_source())
}

#[test]
fn pypi_compatibility_unsupported_target() {
    handle_result(errors::pypi_compatibility_unsupported_target())
}

#[test]
#[cfg(target_os = "linux")]
fn pypi_compatibility_linux_tag() {
    handle_result(errors::pypi_compatibility_linux_tag())
}
