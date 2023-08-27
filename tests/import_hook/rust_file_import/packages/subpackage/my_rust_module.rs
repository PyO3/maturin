use pyo3::prelude::*;
use pyo3::wrap_pyfunction;

#[pyfunction]
pub fn get_num() -> PyResult<usize> {
    Ok(42)
}

#[pymodule]
pub fn my_rust_module(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(get_num))?;
    Ok(())
}
