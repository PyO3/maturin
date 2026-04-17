# cffi-mixed-with-path-dep

This fixture exercises a mixed cffi/python package with a Rust path dependency.

```bash
pip install .
```

```python
import cffi_mixed_with_path_dep

assert cffi_mixed_with_path_dep.lib.get_21() == 21
assert cffi_mixed_with_path_dep.lib.add_21(21) == 42
```

The install smoke test lives in `check_installed/check_installed.py`.
