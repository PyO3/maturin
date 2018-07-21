#!/usr/bin/env bash
# Tested with ubuntu 18.04 with 2.7, 3.5 and 3.6 installed

set -e

cargo run --manifest-path ../Cargo.toml -- build -m $(pwd)/../get_fourtytwo/Cargo.toml

[[ -d venv2.7 ]] || virtualenv -p python2.7 venv2.7

source venv2.7/bin/activate
pip show get_fourtytwo > /dev/null && pip uninstall -y get_fourtytwo
pip install ../target/wheels/get_fourtytwo-*-cp27-cp27mu-manylinux1_x86_64.whl
python main.py
deactivate

[[ -d venv3.5 ]] || virtualenv -p python3.5 venv3.5

source venv3.5/bin/activate
pip show get_fourtytwo > /dev/null && pip uninstall -y get_fourtytwo
pip install ../target/wheels/get_fourtytwo-*-cp35-cp35m-manylinux1_x86_64.whl
python main.py
deactivate

[[ -d venv3.6 ]] || virtualenv -p python3.6 venv3.6

source venv3.6/bin/activate
pip show get_fourtytwo > /dev/null && pip uninstall -y get_fourtytwo
pip install ../target/wheels/get_fourtytwo-*-cp36-cp36m-manylinux1_x86_64.whl
python main.py
deactivate

