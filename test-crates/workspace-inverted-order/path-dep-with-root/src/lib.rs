use pyo3::prelude::*;

#[pymodule]
mod path_dep_with_root {
    use pyo3::pyfunction;
    use top_level::NUMBER;

    #[pyfunction]
    fn add_number(x: u32) -> u32 {
        x + NUMBER
    }
}
