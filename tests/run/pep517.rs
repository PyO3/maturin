use crate::common::handle_result;
use crate::common::pep517::{Pep517Case, target_has_profile, test_pep517};
use std::process::Command;

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

/// Regression test: tracing output must go to stderr, not stdout.
///
/// The PEP 517 protocol uses stdout as a structured channel — pip reads the last
/// line to find the output directory/filename. If tracing leaks to stdout (which
/// happens when RUST_LOG is set and the subscriber writes to stdout), pip fails.
#[test]
fn pep517_tracing_does_not_leak_to_stdout() {
    let output = Command::new(env!("CARGO_BIN_EXE_maturin"))
        // Any subcommand that triggers setup_logging will do; `--help` is
        // handled by clap before logging is initialised, so use `list-python`
        // which is cheap and deterministic.
        .args(["list-python"])
        .env("RUST_LOG", "info")
        .output()
        .expect("failed to run maturin");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Tracing output contains level markers like "INFO", "DEBUG", "TRACE" and
    // timestamps. None of that should appear on stdout. We match without surrounding
    // spaces because ANSI colour codes may be adjacent to the level name.
    for line in stdout.lines() {
        assert!(
            !line.contains("INFO") && !line.contains("DEBUG") && !line.contains("TRACE"),
            "tracing output leaked to stdout: {line:?}\n\nFull stdout:\n{stdout}\nFull stderr:\n{stderr}",
        );
    }

    // Verify that tracing output did land on stderr (sanity check that RUST_LOG was effective).
    assert!(
        stderr.contains("INFO") || stderr.contains("DEBUG") || stderr.contains("TRACE"),
        "expected tracing output on stderr but found none.\nstderr:\n{stderr}",
    );
}
