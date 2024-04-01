use pyo3::prelude::*;

#[pyclass]
struct DummyClass {}

#[pymethods]
impl DummyClass {
    #[staticmethod]
    fn get_42() -> PyResult<usize> {
        Ok(42)
    }
}

/// module level doc string
#[pymodule]
fn pyo3_pure(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<DummyClass>()?;
    m.add("fourtytwo", 42)?;

    Ok(())
}
