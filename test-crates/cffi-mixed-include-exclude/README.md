# cffi-mixed-include-exclude

This fixture exercises a mixed cffi/python package with explicit include/exclude rules.

```bash
pip install .
```

```python
import cffi_mixed_include_exclude

point = cffi_mixed_include_exclude.lib.get_origin()
assert cffi_mixed_include_exclude.lib.is_in_range(point, 0.0)
```

The install smoke test lives in `check_installed/check_installed.py`.
