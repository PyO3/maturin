use pyo3::prelude::*;

#[pyfunction]
fn get_21() -> usize {
    21
}

#[pymodule]
fn pyo3_mixed_src(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(get_21))?;

    Ok(())
}
