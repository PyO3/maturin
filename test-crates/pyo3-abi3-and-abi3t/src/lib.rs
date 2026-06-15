use pyo3::prelude::*;

#[pyfunction]
fn answer() -> PyResult<usize> {
    Ok(42)
}

#[pymodule]
fn pyo3_abi3_and_abi3t(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(answer, m)?)?;
    Ok(())
}
