use crate::common::handle_result;
use crate::common::integration::{self, IntegrationCase};

#[test]
#[cfg_attr(
    all(windows, target_arch = "aarch64"),
    ignore = "possible windows aarch64 pgo issue, see https://github.com/rust-lang/rust/issues/156675"
)]
fn pgo_pyo3_mixed() {
    handle_result(integration::test_integration(
        &IntegrationCase::new("pgo-pyo3-mixed", "test-crates/pyo3-mixed").pgo(),
    ));
}
