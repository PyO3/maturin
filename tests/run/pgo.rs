use crate::common::handle_result;
use crate::common::integration::{self, IntegrationCase};

#[test]
fn pgo_pyo3_mixed() {
    handle_result(integration::test_integration(
        &IntegrationCase::new("pgo-pyo3-mixed", "test-crates/pyo3-mixed").pgo(),
    ));
}
