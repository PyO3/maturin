use pyo3::prelude::*;

#[pyfunction]
fn add(x: usize, y: usize) -> usize {
    let _version = unsafe { libz_sys::zlibVersion() };
    let sum = x + y;
    sum
}

#[pymodule]
fn lib_with_disallowed_lib(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(add))?;

    Ok(())
}
