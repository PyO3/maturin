# cffi-mixed-py-subdir

This fixture exercises a mixed cffi/python package with a Python source subdirectory.

```bash
pip install .
```

```python
import cffi_mixed_py_subdir

point = cffi_mixed_py_subdir.lib.get_origin()
assert cffi_mixed_py_subdir.lib.is_in_range(point, 0.0)
```

The install smoke test lives in `check_installed/check_installed.py`.
