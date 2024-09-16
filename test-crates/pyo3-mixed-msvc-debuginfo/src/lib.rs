use pyo3::prelude::*;

/// A Python module implemented in Rust.
#[pymodule]
// keep the name the same as `module-name = "pyo3_mixed._pyo3_lib"` in `pyproject.toml`
#[pyo3(name = "_pyo3_lib")]
mod pyo3_lib {
    use super::*;

    /// Formats the sum of two numbers as string.
    #[pyfunction]
    fn sum_as_string(a: usize, b: usize) -> PyResult<String> {
        Ok((a + b).to_string())
    }
}
