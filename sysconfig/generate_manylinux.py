import os
import sys
import json
from collections import defaultdict
import subprocess


ARCHES = ["x86_64", "i686", "aarch64", "ppc64le", "s390x"]
PY_VERS = [
    "cp36-cp36m",
    "cp37-cp37m",
    "cp38-cp38",
    "cp39-cp39",
    "cp310-cp310",
    "pp37-pypy37_pp73",
    "pp38-pypy38_pp73",
    "pp39-pypy39_pp73",
]


def main():
    well_known = defaultdict(list)
    cwd = os.getcwd()
    for arch in ARCHES:
        docker_image = f"quay.io/pypa/manylinux2014_{arch}"
        for ver in PY_VERS:
            # PyPy is not available on ppc64le & s390x
            if arch in ["ppc64le", "s390x"] and ver.startswith("pp"):
                continue
            command = [
                "docker",
                "run",
                "--rm",
                "-it",
                "-v",
                f"{cwd}:/io",
                "-w",
                "/io",
                docker_image,
                f"/opt/python/{ver}/bin/python",
                "/io/src/python_interpreter/get_interpreter_metadata.py",
            ]
            try:
                metadata = subprocess.check_output(command).decode().strip()
            except subprocess.CalledProcessError as exc:
                print(exc.output, file=sys.stderr)
                raise
            metadata = json.loads(metadata.splitlines()[-1])
            for key in ["system", "platform"]:
                metadata.pop(key, None)
            well_known[arch].append(metadata)

    with open("src/python_interpreter/sysconfig-linux.json", "w") as f:
        f.write(json.dumps(well_known, indent=2))


if __name__ == "__main__":
    main()
