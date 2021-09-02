# pyo3-mixed-submodule

A package for testing maturin with a mixed pyo3/python project with Rust submodule.

## Usage

```bash
pip install .
```

```python
import pyo3_mixed_submodule
assert pyo3_mixed_submodule.get_42() == 42
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

The tests are in `tests/test_pyo3_mixed_submodule.py`, while the configuration is in tox.ini
