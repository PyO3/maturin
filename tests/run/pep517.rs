use crate::common::handle_result;
use crate::common::pep517::{Pep517Case, target_has_profile, test_pep517};

#[test]
fn pep517_default_profile() {
    let case = Pep517Case::new("pep517-pyo3-pure", "test-crates/pyo3-pure");
    handle_result(test_pep517(&case));

    assert!(target_has_profile(case.id, "release"));
    assert!(!target_has_profile(case.id, "debug"));
}

#[test]
fn pep517_editable_profile() {
    let case = Pep517Case::new("pep517-pyo3-pure-editable", "test-crates/pyo3-pure").editable();
    handle_result(test_pep517(&case));

    assert!(!target_has_profile(case.id, "release"));
    assert!(target_has_profile(case.id, "debug"));
}
