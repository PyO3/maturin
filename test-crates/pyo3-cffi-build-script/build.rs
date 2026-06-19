//! Models the `cryptography` project: a PyO3 extension whose build script shells
//! out to the build interpreter to run cffi. maturin must hand us that
//! interpreter via `PYO3_PYTHON`, even for abi3 builds.

use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
    let python = env::var("PYO3_PYTHON")
        .expect("PYO3_PYTHON is not set; maturin must hand abi3 builds an interpreter");
    let status = Command::new(&python)
        .args([
            "-c",
            "import cffi; ffi = cffi.FFI(); ffi.cdef('int answer(void);'); \
             ffi.set_source('_pyo3_cffi_demo', 'int answer(void) { return 42; }')",
        ])
        .status()
        .expect("failed to run PYO3_PYTHON");
    assert!(status.success(), "cffi codegen via PYO3_PYTHON failed");
}
