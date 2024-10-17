use pyo3::prelude::*;

#[pymodule]
mod path_dep_with_root {
    use pyo3::prelude::*;
    use top_level::NUMBER;

    #[pymodule_init]
    #[allow(unused_variables)]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        println!("Hi from rust {NUMBER}");
        Ok(())
    }
}
