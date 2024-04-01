use pyo3::prelude::*;

use pyo3_mixed_workspace::get_21_lib;

#[pyfunction]
fn get_21() -> usize {
    get_21_lib()
}

#[pymodule]
fn pyo3_mixed_workspace_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(get_21, m)?)?;

    Ok(())
}
