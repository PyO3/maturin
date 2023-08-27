use pyo3::prelude::*;

#[pymodule]
fn blank_project(_py: Python, _m: &PyModule) -> PyResult<()> {
    Ok(())
}
