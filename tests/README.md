# Test Running Notes
- `virtualenv` is required to be in the PATH when running the tests
- intermediate test artifacts are written to `test-crates/targets` and `test-crates/venvs`.
  keeping them may speed up running the tests again, but you may also remove them once the tests have finished.
- the import hook tests cannot easily be run outside the test runner.
- to run a single import hook test, modify the test runner in `run.rs` to specify a single test instead of a whole module. For example:
- set `CLEAR_WORKSPACE=False` if you want to inspect the output after a test has run
- include "debugpy" in the list of packages if you want to use a debugger such as vscode to place breakpoints and debug:
    - `import debugpy; debugpy.listen(5678); debugpy.wait_for_client(); debugpy.breakpoint()`

```rust
handle_result(import_hook::test_import_hook(
    "import_hook_rust_file_importer",
    "tests/import_hook/test_rust_file_importer.py::test_multiple_imports",  // <--
    &["boltons", "debugpy"], // <--
    &[("MATURIN_TEST_NAME", "ALL")],
    true,
));
```

## Debugging The Import Hook
- if an individual package is failing to import for some reason
  - configure the logging level to get more information from the import hook.
  - create a script that calls `maturin.import_hook.install()` and run the script in a debugger and step into the import hook source code
- to debug the rust implementation of resolving projects, create and run a test like so. Run the test in a debugger or add print statements.

```rust
#[test]
fn test_resolve_package() {
    debug_print_resolved_package(&Path::new("test-crates/pyo3-mixed-workspace"));
}
```
