# Get fourtytwo

A package with pyo3 bindings for testing maturin.

## Usage

```bash
pip install .
```

```python
import pyo3_pure
assert pyo3_pure.DummyClass.get_42() == 42
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

The tests are in `test_pyo3_pure.py`, while the configuration is in tox.ini
