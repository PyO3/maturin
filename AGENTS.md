# AGENTS.md

## Project overview

maturin builds and publishes Rust crates with pyo3, cffi, and uniffi bindings (and standalone Rust binaries) as Python packages. The core is written in Rust (`src/`) and exposed both as a CLI (`maturin`) and as a PEP 517 build backend through a small Python shim (`maturin/`).

- `src/` — Rust source for the CLI, build backend, and library crate.
- `maturin/` — Python package: PEP 517 bootstrap backend, `__main__` shim, and the import hook.
- `tests/` — Rust integration tests (`cli.rs`, `run.rs`, helpers in `common/`, `cmd/`).
- `test-crates/` — Fixture crates exercised by integration tests; do not commit changes here unless updating fixtures intentionally.
- `guide/` — mdBook user documentation published at https://maturin.rs.
- `sysconfig/` — Captured Python sysconfigs used for cross-compilation; treat as data.
- `src/templates/` — `cargo-generate` templates for `maturin new`/`init`. Excluded from formatters/linters.
- `noxfile.py` — Maintenance automation (e.g. `update-pyo3`, pyodide setup).

## Build & run

- Build the CLI: `cargo build` (or `cargo build --release`).
- Run the CLI from source: `cargo run -- <args>`.

## Tests

- Rust unit + integration tests: `cargo test --all-features`.
- A single integration test: `cargo test --test run -- <substring>`.
- Python tests: `pytest tests/` (some require a built `maturin` on `PATH`).
- Long-running cross-compile / manylinux tests live in `tests/manylinux_*.sh` and `tests/run.rs`; many require Docker, zig, or specific Python interpreters and are skipped by default.

Always run the relevant tests for code you touched and report failures faithfully.

## Lint & format

Before finishing a change, run:

- `cargo fmt --all`
- `cargo clippy --tests --all-features -- -D warnings`
- `cargo deny --all-features check` (when touching dependencies)
- `pre-commit run --all-files` for Python / general hooks; `pre-commit run --hook-stage manual --all-files` to also run cargo-check and clippy.

Python code uses `ruff` and `black` (line length 120, target py37) and `mypy` with strict untyped-def checks; see `pyproject.toml`.

## Conventions

- Rust edition and MSRV are pinned in `Cargo.toml` (`rust-version`); do not bump casually.
- Keep public CLI flags and `pyproject.toml` `[tool.maturin]` keys backward compatible.
- When adding a CLI option, regenerate the JSON schema via `cargo run --bin generate_json_schema` (see `src/generate_json_schema.rs`) so `maturin.schema.json` stays in sync.
- Do not edit files under `src/templates/` to satisfy formatters; they are intentionally excluded.
- Do not modify `sysconfig/` snapshots by hand.
- Do not modify `Changelog` by hand, it's managed by `cargo-cliff`.
- Document what & why in git commit message instead of just a list of changes, use backtick for code snippets in commit message.
