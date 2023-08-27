use pyo3::prelude::*;
use some_path_dep::{add, is_sum};

#[pyfunction]
fn get_21() -> usize {
    21
}

#[pyfunction]
fn add_21(num: usize) -> usize {
    add(num, get_21())
}

#[pyfunction]
fn is_half(a: usize, b: usize) -> bool {
    is_sum(a, a, b)
}


#[pymodule]
fn pyo3_mixed_with_path_dep(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(get_21))?;
    m.add_wrapped(wrap_pyfunction!(add_21))?;
    m.add_wrapped(wrap_pyfunction!(is_half))?;

    Ok(())
}
