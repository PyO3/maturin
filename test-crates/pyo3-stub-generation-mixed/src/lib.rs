use pyo3::prelude::*;

#[pymodule]
mod pyo3_stub_generation_mixed {
    use pyo3::prelude::*;

    #[pyfunction]
    fn get_21() -> usize {
        21
    }

    #[pymodule]
    mod submodule {
        use pyo3::prelude::*;

        #[pyfunction]
        fn get_42() -> usize {
            42
        }
    }
}
