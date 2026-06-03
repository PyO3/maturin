use pyo3::prelude::*;

#[pymodule]
mod _pyo3_mixed {
    use pyo3::prelude::*;

    #[pyfunction]
    fn get_21() -> usize {
        21
    }
}
