# cffi-pure

A package for testing a crate with cffi bindings with maturin.

Read the [cffi guide](https://cffi.readthedocs.io/en/latest/index.html) to learn how to use the generated `ffi` and `lib` objects.

## Usage

```bash
pip install .
```

```python
import cffi_pure

point = cffi_pure.lib.get_origin()
point.x = 10
point.y = 10
assert cffi_pure.lib.is_in_range(point, 15)
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

The tests are in `test_cffi_pure.py`, while the configuration is in tox.ini
