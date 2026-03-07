use crate::common::{handle_result, other};
use serial_test::serial;
use std::env;
use time::macros::datetime;

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn musl() {
    let ran = handle_result(other::test_musl());
    if !ran {
        eprintln!("Warning: rustup and/or musl target not installed, test didn't run");
    }
}

#[test]
#[cfg(unix)]
fn unreadable_dir() {
    handle_result(other::test_unreadable_dir())
}

#[test]
#[serial(source_date_epoch_env)]
fn pyo3_source_date_epoch() {
    unsafe { env::set_var("SOURCE_DATE_EPOCH", "0") };
    handle_result(other::check_wheel_mtimes(
        "test-crates/pyo3-mixed-include-exclude",
        vec![datetime!(1980-01-01 0:00)],
        "pyo3_source_date_epoch",
    ))
}

#[test]
#[serial(source_date_epoch_env)]
fn sdist_no_source_date_epoch() {
    unsafe { env::remove_var("SOURCE_DATE_EPOCH") };
    handle_result(other::check_sdist_mtimes(
        "test-crates/pyo3-mixed-include-exclude",
        1153704088,
        "sdist_no_source_date_epoch",
    ))
}

#[test]
#[serial(source_date_epoch_env)]
fn sdist_source_date_epoch() {
    unsafe { env::set_var("SOURCE_DATE_EPOCH", "1") };
    handle_result(other::check_sdist_mtimes(
        "test-crates/pyo3-mixed-include-exclude",
        1,
        "sdist_source_date_epoch",
    ))
}
