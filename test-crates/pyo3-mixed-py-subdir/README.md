# pyo3-mixed

A package for testing maturin with a mixed pyo3/python project and a non-default package name.

## Usage

```bash
pip install .
```

```python
import pyo3_mixed_py_subdir
assert pyo3_mixed_py_subdir.get_42() == 42
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

The tests are in `test_pyo3_mixed.py`, while the configuration is in tox.ini
