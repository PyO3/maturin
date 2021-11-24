use pyo3::prelude::*;

#[pymodule]
fn {{crate_name}}(_py: Python, m: &PyModule) -> PyResult<()> {
    Ok(())
}
