use pyo3::prelude::*;

#[pymodule]
mod pyo3_stub_generation {
    use super::*;

    #[pyclass]
    struct Class {}

    #[pyfunction]
    fn func(a: usize) -> usize {
        a
    }

    #[pymodule]
    mod submodule {
        use super::*;

        #[pyclass]
        struct Class2 {}
    }
}
