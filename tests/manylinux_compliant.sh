#!/bin/bash
set -e

for PYBIN in /opt/python/cp3[89]*/bin; do
  $1 build -m test-crates/pyo3-mixed/Cargo.toml --target-dir test-crates/targets -i "${PYBIN}/python" --manylinux $2 -o dist
done
