# cffi-mixed-implicit

This fixture exercises a mixed cffi/python package with an implicit namespace package
and a Rust-backed submodule.

```bash
pip install .
```

```python
import cffi_mixed_implicit.some_rust as some_rust

point = some_rust.lib.get_origin()
assert some_rust.lib.is_in_range(point, 0.0)
```

The install smoke test lives in `check_installed/check_installed.py`.
