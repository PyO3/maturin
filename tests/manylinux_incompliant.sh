#!/bin/bash

# Fail because we're running in manylinux2014, which can't build for manylinux 2010
for PYBIN in /opt/python/cp3[9]*/bin; do
  if cargo run -- build --no-sdist -m test-crates/pyo3-mixed/Cargo.toml -i "${PYBIN}/python" --manylinux 2010 -o dist; then
    echo "maturin build unexpectedly succeeded"
    exit 1
  else
    echo "maturin build failed as expected"
  fi
done

# Fail because we're linking zlib with black-listed symbols(gzflags), which is not allowed in manylinux
yum install -y zlib-devel
for PYBIN in /opt/python/cp3[9]*/bin; do
  if cargo run -- build --no-sdist -m test-crates/lib_with_disallowed_lib/Cargo.toml -i "${PYBIN}/python" --manylinux 2014 -o dist; then
    echo "maturin build unexpectedly succeeded"
    exit 1
  else
    echo "maturin build failed as expected"
  fi
done

# Fail because manylinux_2_99 policy is not defined by auditwheel
for PYBIN in /opt/python/cp3[9]*/bin; do
  if cargo run -- build --no-sdist -m test-crates/pyo3-mixed/Cargo.toml -i "${PYBIN}/python" --compatibility manylinux_2_99 -o dist; then
    echo "maturin build unexpectedly succeeded"
    exit 1
  else
    echo "maturin build failed as expected"
  fi
done
