set -euxo pipefail

sudo apt update
sudo apt install -y python3-dev python3-pip python3-venv libclang-dev
sudo python3 -m pip install cffi virtualenv uniffi-bindgen
