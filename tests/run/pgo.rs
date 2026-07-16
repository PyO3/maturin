use crate::common::integration::{self, IntegrationCase};
use crate::common::{handle_result, has_uv};

#[test]
#[cfg_attr(
    all(windows, target_arch = "aarch64"),
    ignore = "possible windows aarch64 pgo issue, see https://github.com/rust-lang/rust/issues/156675"
)]
fn pgo_pyo3_mixed() {
    // Allow CI jobs without llvm-tools (e.g. Alpine, which uses system rust) to opt out.
    if std::env::var_os("MATURIN_TEST_SKIP_PGO").is_some() {
        return;
    }
    handle_result(integration::test_integration(
        &IntegrationCase::new("pgo-pyo3-mixed", "test-crates/pyo3-mixed").pgo(),
    ));
}

#[test]
#[cfg_attr(
    all(windows, target_arch = "aarch64"),
    ignore = "possible windows aarch64 pgo issue, see https://github.com/rust-lang/rust/issues/156675"
)]
fn pgo_bin() {
    if std::env::var_os("MATURIN_TEST_SKIP_PGO").is_some() || !has_uv() {
        return;
    }
    handle_result(integration::test_integration(
        &IntegrationCase::new("pgo-bin", "test-crates/hello-world").pgo(),
    ));
}

#[test]
#[cfg_attr(
    all(windows, target_arch = "aarch64"),
    ignore = "possible windows aarch64 pgo issue, see https://github.com/rust-lang/rust/issues/156675"
)]
fn pgo_pyo3_bin_uv_multi_python() {
    if std::env::var_os("MATURIN_TEST_SKIP_PGO").is_some() || !has_uv() {
        return;
    }
    handle_result(integration::test_integration_uv_multi_python(
        &IntegrationCase::new("pgo-pyo3-bin-uv-multi-python", "test-crates/pyo3-bin").pgo(),
    ));
}

#[test]
fn pgo_bin_uv_multi_python() {
    if std::env::var_os("MATURIN_TEST_SKIP_PGO").is_some() || !has_uv() {
        return;
    }
    let err = integration::test_integration_uv_multi_python(
        &IntegrationCase::new("pgo-bin-uv-multi-python", "test-crates/hello-world").pgo(),
    )
    .expect_err("multiple interpreters is not applicable with non-pyo3 binaries");
    assert_eq!(
        err.to_string(),
        "You can only specify one python interpreter for `bin` bindings"
    );
}
