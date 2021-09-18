use std::os::raw::c_ulong;

use pyo3::prelude::*;

#[link(name = "z")]
extern "C" {
    fn gzflags() -> c_ulong;
}

#[pyfunction]
fn add(x: usize, y: usize) -> usize {
    let _version = unsafe { libz_sys::zlibVersion() };
    let _flags = unsafe { gzflags() };
    let sum = x + y;
    sum
}

#[pymodule]
fn lib_with_disallowed_lib(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(add))?;

    Ok(())
}
