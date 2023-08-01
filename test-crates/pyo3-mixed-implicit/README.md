# pyo3-mixed-implicit

A package for testing maturin with a mixed pyo3/python project with an implicit namespace package and Rust submodule.

## Usage

```bash
pip install .
```

```python
import pyo3_mixed_implicit
assert pyo3_mixed_implicit.some_rust.get_22() == 22
```

## Testing

Install tox:

```bash
pip install tox
```

Run it:

```bash
tox
```

The tests are in `tests/test_pyo3_mixed_implicit.py`, while the configuration is in tox.ini
