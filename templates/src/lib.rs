{%- case binding -%}
{%- when "pyo3" -%}
use pyo3::prelude::*;

#[pymodule]
fn {{crate_name}}(_py: Python, m: &PyModule) -> PyResult<()> {
    Ok(())
}
{%- when "rust-cpython" -%}
use cpython::py_module_initializer;

py_module_initializer!({{crate_name}}, |py, m| {
    m.add(py, "__doc__", "Module documentation string")?;
    Ok(())
});
{%- else -%}
{% endcase %}
