use crate::common::handle_result;
use crate::common::integration::{self, IntegrationCase};

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
