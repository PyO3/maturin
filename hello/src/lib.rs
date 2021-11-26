
use pyo3::prelude::*;

#[pymodule]
fn hello(_py: Python, m: &PyModule) -> PyResult<()> {
    Ok(())
}
