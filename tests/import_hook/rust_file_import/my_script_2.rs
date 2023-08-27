use pyo3::prelude::*;

#[pyfunction]
fn get_num() -> usize { 20 }

#[pyfunction]
fn get_other_num() -> usize { 100 }

#[pymodule]
fn my_script(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(get_num))?;
    m.add_wrapped(wrap_pyfunction!(get_other_num))?;
    Ok(())
}
