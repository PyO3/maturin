use pyo3::prelude::*;

#[pyfunction]
fn get_21() -> usize {
    21
}

#[pymodule]
fn _pyo3_mixed(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(get_21))?;

    Ok(())
}
