# pyo3-mixed-include-exclude

A package for testing maturin with a mixed pyo3/python project with include and exclude configurations.

## Usage

```bash
pip install .
```

```python
import pyo3_mixed_include_exclude
assert pyo3_mixed_include_exclude.get_42() == 42
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

The tests are in `test_pyo3_mixed_include_exclude.py`, while the configuration is in tox.ini
