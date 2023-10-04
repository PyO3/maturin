use pyo3::prelude::*;

#[pyfunction]
fn get_22() -> usize {
    22
}

#[pymodule]
fn rust(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(get_22))?;

    Ok(())
}
