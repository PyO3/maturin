use pyo3::prelude::*;

#[pymodule]
fn pyo3_cffi_build_script(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("answer", 42)?;
    Ok(())
}
