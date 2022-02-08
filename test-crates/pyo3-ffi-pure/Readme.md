# pyo3-ffi-pure

A package with pyo3-ffi bindings for testing maturin.

## Usage

```python
import pyo3_ffi_pure
assert pyo3_ffi_pure.sum(2, 40) == 42
```

## Testing

Install `pytest` and run:

```bash
pytest -v test_pyo3_ffi_pure.py
```
