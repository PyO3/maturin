use pyo3::{pymodule, Bound, PyResult};
use pyo3::types::{PyModule, PyModuleMethods};

#[pymodule]
fn readme(m: &Bound<PyModule>) -> PyResult<()> {
    m.add("value", 1)?;
    Ok(())
}
