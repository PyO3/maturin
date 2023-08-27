use pyo3::prelude::*;

#[pyfunction]
fn get_num() -> usize {
    if cfg!(feature = "large_number") {
        100
    } else {
        10
    }
}

#[pymodule]
fn my_script(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(get_num))?;
    Ok(())
}
