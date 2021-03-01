#!/bin/bash

set -ex

# The problem with testing this is that we need to go through the PEP 517 so we need a wheel of maturin,
# which makes everything complex and slow
cd "$(git rev-parse --show-toplevel)" # Go to project root

pip uninstall -y lib_with_path_dep 2> /dev/null
# Make sure it's actually removed
python -c "from lib_with_path_dep import add; assert add(2,2)==4" 2> /dev/null && exit 1 || true

# Build maturin wheel
cargo run -- build -b bin --strip --manylinux off
cargo run -- pep517 write-sdist --manifest-path test-crates/lib_with_path_dep/Cargo.toml --sdist-directory test-crates/lib_with_path_dep/target/wheels
# Slower alternative: cargo run -- build -m test-crates/lib_with_path_dep/Cargo.toml -i python
# See https://github.com/pypa/pip/issues/6041

# First, use the maturin wheel we just build
# Then install lib_with_path_dep from the sdist we build
pip install \
  --find-links target/wheels/ \
  --force-reinstall --no-binary lib_with_path_dep --find-links test-crates/lib_with_path_dep/target/wheels lib_with_path_dep
python -c "from lib_with_path_dep import add; assert add(2,2)==4"
