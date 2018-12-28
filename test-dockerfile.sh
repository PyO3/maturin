#!/usr/bin/env bash
# Builds each of the three examples (cffi, binary and pyo3) using the docker
# container, installs the wheel and checks that the installed package is functional

set -e

rm -r venv-docker

python3.6 -m venv venv-docker

source venv-docker/bin/activate

pip install cffi > /dev/null

docker run --rm -v $(pwd)/hello-world:/io pyo3-pack build -b bin

pip install hello-world --no-index --find-links hello-world/target/wheels/

if [[ $(python hello-world/check_installed.py) != 'SUCCESS' ]]; then
  exit 1
fi

docker run --rm -v $(pwd)/points:/io pyo3-pack build -b cffi

pip install points --no-index --find-links points/target/wheels/

if [[ $(python points/check_installed.py) != 'SUCCESS' ]]; then
  exit 1
fi

docker run --rm -v $(pwd)/get-fourtytwo:/io pyo3-pack build -i python3.6

pip install get-fourtytwo --no-index --find-links get-fourtytwo/target/wheels/

if [[ $(python points/check_installed.py) != 'SUCCESS' ]]; then
  exit 1
fi

deactivate
