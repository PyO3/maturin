# cffi-mixed

A package for testing maturin with a cffi wrapper with a rust backend and a python wrapper.

Read the [cffi guide](https://cffi.readthedocs.io/en/latest/index.html) to learn how to use the generated `ffi` and `lib` objects.

The package contains a `Point` type implemented in rust and a `Line` class consisting of two points implemented in python

## Usage

```bash
pip install .
```

```python
import cffi_mixed

from cffi_mixed import Line

point = cffi_mixed.lib.get_origin()
point.x = 10
point.y = 10
assert cffi_mixed.lib.is_in_range(point, 15)

assert Line(2, 5, 6, 8).length() == 5
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

The tests are in `test_cffi_mixed.py`, while the configuration is in tox.ini
