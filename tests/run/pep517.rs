use crate::common::pep517::{Pep517Case, target_has_profile, test_pep517};
use crate::common::{TestEnvKind, handle_result};

#[test]
fn pep517_default_profile() {
    let case = Pep517Case {
        id: "pep517-pyo3-pure",
        package: "test-crates/pyo3-pure",
        env_kind: TestEnvKind::Venv,
        editable: false,
        prereq_packages: &[],
    };
    handle_result(test_pep517(&case));

    assert!(target_has_profile(case.id, "release"));
    assert!(!target_has_profile(case.id, "debug"));
}

#[test]
fn pep517_editable_profile() {
    let case = Pep517Case {
        id: "pep517-pyo3-pure-editable",
        package: "test-crates/pyo3-pure",
        env_kind: TestEnvKind::Venv,
        editable: true,
        prereq_packages: &[],
    };
    handle_result(test_pep517(&case));

    assert!(!target_has_profile(case.id, "release"));
    assert!(target_has_profile(case.id, "debug"));
}
