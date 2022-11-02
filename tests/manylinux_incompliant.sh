#!/bin/bash
set -e

which cargo > /dev/null || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal

# Fail because we're running in manylinux2014, which can't build for manylinux 2010
for PYBIN in /opt/python/cp3[9]*/bin; do
  if cargo run -- build -m test-crates/pyo3-mixed/Cargo.toml --target-dir test-crates/targets -i "${PYBIN}/python" --manylinux 2010 -o dist; then
    echo "maturin build unexpectedly succeeded"
    exit 1
  else
    echo "maturin build failed as expected"
  fi
done

# Fail because we're linking zlib with black-listed symbols(gzflags), which is not allowed in manylinux
apt-get -v &> /dev/null && apt-get install -y zlib1g-dev || yum install -y zlib-devel

for PYBIN in /opt/python/cp3[9]*/bin; do
  if cargo run -- build -m test-crates/lib_with_disallowed_lib/Cargo.toml --target-dir test-crates/targets -i "${PYBIN}/python" --manylinux 2014 -o dist; then
    echo "maturin build unexpectedly succeeded"
    exit 1
  else
    echo "maturin build failed as expected"
  fi
done
