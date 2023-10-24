# Test Running Notes
- `virtualenv` is required to be in the PATH when running the tests
- intermediate test artifacts are written to `test-crates/targets` and `test-crates/venvs`.
  keeping them may speed up running the tests again, but you may also remove them once the tests have finished.
- the import hook tests cannot easily be run outside the test runner.
- to run a single import hook test, modify the test runner in `run.rs` to specify a single test instead of a whole module. For example:

```rust
handle_result(import_hook::test_import_hook(
    "import_hook_rust_file_importer",
    "tests/import_hook/test_rust_file_importer.py::test_multiple_imports",  // <--
    &[],
    &[("MATURIN_TEST_NAME", "ALL")],
    true,
));
```
