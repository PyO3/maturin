## Code Standards

### Required Before Each Commit

- Run `cargo clippy --tests --all-features -- -D warnings` to ensure the code builds successfully and address any warnings.
- Run `cargo fmt` to ensure the code is formatted correctly.
- Run `cargo test` to ensure all tests pass.
- Run `pre-commit` to ensure code quality and consistency.

## Key Guidelines

- Follow Rust best practices and idiomatic patterns.
- Maintain existing code structure and organization by default unless instructed to refactor or refactoring can improve maintainability or performance.
