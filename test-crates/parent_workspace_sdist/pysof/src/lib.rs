use pyo3::prelude::*;

#[pyfunction]
fn get_value() -> i32 {
    shared_crate::shared_function()
}

#[pymodule]
fn pysof(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(get_value, m)?)?;
    Ok(())
}
