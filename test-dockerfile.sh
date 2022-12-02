#!/usr/bin/env bash
# Builds all 6 test crates using the docker container,
# installs the wheel and checks that the installed package is functional

set -e

rm -rf venv-docker
python3.11 -m venv venv-docker
venv-docker/bin/pip install -U pip cffi

# FIXME: Can we run the tests without activate? Currently hello-world fails because then the binary is not in PATH
source venv-docker/bin/activate

for test_crate in hello-world cffi-pure cffi-mixed pyo3-pure pyo3-mixed pyo3-mixed-submodule
do
  echo "Testing $test_crate"
  docker run -e RUST_BACKTRACE=1 --rm -v "$(pwd):/io" -w /io/test-crates/$test_crate maturin build -i python3.11
  # --only-binary=:all: stops pip from picking a local already compiled sdist
  venv-docker/bin/pip install $test_crate --only-binary=:all: --find-links test-crates/$test_crate/target/wheels/
  if [[ $(venv-docker/bin/python test-crates/$test_crate/check_installed/check_installed.py) != 'SUCCESS' ]]; then
    exit 1
  fi
done

deactivate
