# cffi-mixed-submodule

This fixture exercises a mixed cffi/python package with a Rust-backed submodule.

```bash
pip install .
```

```python
from cffi_mixed_submodule.rust_module import rust

point = rust.lib.get_origin()
assert rust.lib.is_in_range(point, 0.0)
```

The install smoke test lives in `check_installed/check_installed.py`.
