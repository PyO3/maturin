#!/bin/bash

for PYBIN in /opt/python/cp3[6789]*/bin; do
  cargo run -- build --no-sdist -m test-crates/pyo3-mixed/Cargo.toml -i "${PYBIN}/python" --manylinux $1 -o dist
done
