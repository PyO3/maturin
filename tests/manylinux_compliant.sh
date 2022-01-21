#!/bin/bash
which cargo > /dev/null 2>&1 || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal

for PYBIN in /opt/python/cp3[6789]*/bin; do
  cargo run -- build --no-sdist -m test-crates/pyo3-mixed/Cargo.toml -i "${PYBIN}/python" --manylinux $1 -o dist
done
