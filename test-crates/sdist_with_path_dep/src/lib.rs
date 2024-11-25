use pyo3::prelude::*;

#[pyfunction]
fn add(x: usize, y: usize) -> usize {
    let sum = some_path_dep::add(x, y);
    debug_assert!(some_path_dep::is_sum(x, y, sum));
    sum
}

#[pymodule]
fn sdist_with_path_dep(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(add))?;
    Ok(())
}
