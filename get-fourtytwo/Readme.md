# Get fourty_two

A dummy package for testing pyo3-pack.

## Usage

```bash
pip install --index-url https://test.pypi.org/simple/ get_fourtytwo
```

```python
import get_fourtytwo
assert get_fourtytwo.DummyClass.get_42() == 42
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

The tests are in `test_get_gourtytwo.py`, while the configuration is in tox.ini
