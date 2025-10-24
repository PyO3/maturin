use pyo3::prelude::*;

fn main() -> PyResult<()> {
    Python::attach(|py| {
        let builtins = py.import("builtins")?;
        let total: i32 = builtins.getattr("sum")?.call1((vec![1, 2, 3],))?.extract()?;
        assert_eq!(total, 6);
        println!("Hello, world!");
        Ok(())
    })
}
