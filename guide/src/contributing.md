# Contributing

Contributions are welcome, and they are greatly appreciated!

You can contribute in many ways:

## Types of Contributions

### Report Bugs

Report bugs at [https://github.com/PyO3/maturin/issues](https://github.com/PyO3/maturin/issues).

### Fix Bugs

Look through the GitHub issues for bugs. Anything tagged with `bug`
and `help wanted` is open to whoever wants to implement it.

### Implement Features

Look through the GitHub issues for features.

### Write Documentation

Maturin could always use more documentation, whether as part of the official
guide, in docstrings or even on the web in blog posts, articles and such.

### Submit Feedback

The best way to send feedback is to start a new discussion
at [https://github.com/PyO3/maturin/discussions](https://github.com/PyO3/maturin/discussions).

## Get Started!

Ready to contribute? Here's how to setup maturin for local development.

1. Fork the maturin repository on GitHub.
2. Clone your fork locally:
   ```bash
   $ git clone git@github.com:your_name_here/maturin.git
   ```
3. [Install a stable Rust toolchain](https://www.rust-lang.org/tools/install)
   and of course [Python 3.6 or later is also required](https://realpython.com/installing-python/).
4. Create a branch for local development:
   ```bash
   $ cd maturin
   $ git checkout -b branch-name
   ```
   Now you can make your changes locally.
5. When you're done making changes, format your changes with `cargo fmt`, then
   lint with `cargo clippy` and test them with `cargo test`:
   ```bash
   $ cargo fmt
   $ cargo clippy
   $ cargo test
   ```
   Note that in order to run tests you need to install `virtualenv` and
   `cffi` (`pip3 install cffi virtualenv`).
6. Commit your changes and push your branch to GitHub:
   ```bash
   $ git add .
   $ git Commit
   $ git push origin branch-name
   ```
7. Submit a pull request through the [GitHub website](https://github.com/PyO3/maturin/pulls).

## Pull Request Guidelines

Before you submit a pull request, check that it meets these guidelines:

1. The pull request should include tests if it adds or changes functionalities.
2. Add a [changelog](https://github.com/PyO3/maturin/blob/main/Changelog.md)
   entry.
3. When command line interface changes, run `python3 test-crates/update_readme.py` to update related documentation.
