# cffi-mixed-src

This fixture exercises a src-layout mixed cffi/python package.

```bash
pip install .
```

```python
import cffi_mixed_src

point = cffi_mixed_src.lib.get_origin()
assert cffi_mixed_src.lib.is_in_range(point, 0.0)
```

The install smoke test lives in `check_installed/check_installed.py`.
