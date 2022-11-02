#!/bin/bash
set -e

which cargo > /dev/null 2>&1 || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal

for PYBIN in /opt/python/cp3[789]*/bin; do
  cargo run -- build -m test-crates/pyo3-mixed/Cargo.toml --target-dir test-crates/targets -i "${PYBIN}/python" --manylinux $1 -o dist
done
