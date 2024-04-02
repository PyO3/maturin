use pyo3::prelude::*;

#[pyfunction]
fn get_22() -> usize {
    22
}

#[pymodule]
fn rust(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(get_22))?;

    Ok(())
}
