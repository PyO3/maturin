use pyo3::prelude::*;
use std::env;

#[pyfunction]
fn get_21() -> usize {
    21
}

/// Prints the CLI arguments, once from Rust's point of view and once from Python's point of view.
#[pyfunction]
fn print_cli_args(py: Python) -> PyResult<()> {
    // This one includes Python and the name of the wrapper script itself, e.g.
    // `["/home/ferris/.venv/bin/python", "/home/ferris/.venv/bin/print_cli_args", "a", "b", "c"]`
    println!("{:?}", env::args().collect::<Vec<_>>());
    // This one includes only the name of the wrapper script itself, e.g.
    // `["/home/ferris/.venv/bin/print_cli_args", "a", "b", "c"])`
    println!(
        "{:?}",
        py.import("sys")?
            .getattr("argv")?
            .extract::<Vec<String>>()?
    );
    Ok(())
}

#[pymodule]
fn pyo3_mixed(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(get_21))?;
    m.add_wrapped(wrap_pyfunction!(print_cli_args))?;

    Ok(())
}
