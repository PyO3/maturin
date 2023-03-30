import json
import platform
import sys
import sysconfig
import struct

if platform.python_implementation() == "PyPy":
    # Workaround for PyPy 3.6 on windows:
    #  - sysconfig.get_config_var("EXT_SUFFIX") differs to importlib until
    #    Python 3.8
    #  - PyPy does not load the plain ".pyd" suffix because it expects that's
    #    for a CPython extension module
    #
    # This workaround can probably be removed once PyPy for Python 3.8 is the
    # main PyPy version.
    import importlib.machinery

    ext_suffix = importlib.machinery.EXTENSION_SUFFIXES[0]
else:
    ext_suffix = sysconfig.get_config_var("EXT_SUFFIX")


def get_abi_tag():
    # This should probably return the ABI tag based on EXT_SUFFIX in the same
    # way as pypa/packaging. See https://github.com/pypa/packaging/pull/607.
    # For simplicity, we just fix it up for GraalPy for now and leave the logic
    # for the other interpreters untouched, but this should be fixed properly
    # down the road.
    if platform.python_implementation() == "GraalVM":
        ext_suffix = sysconfig.get_config_var("EXT_SUFFIX")
        parts = ext_suffix.split(".")
        soabi = parts[1]
        abi = "-".join(soabi.split("-")[:3])
        return abi.replace(".", "_").replace("-", "_")
    else:
        return (sysconfig.get_config_var("SOABI") or "-").split("-")[1] or None,


metadata = {
    # sys.implementation.name can differ from platform.python_implementation(), for example
    # Pyston has sys.implementation.name == "pyston" while platform.python_implementation() == cpython
    "implementation_name": sys.implementation.name,
    "executable": sys.executable or None,
    "major": sys.version_info.major,
    "minor": sys.version_info.minor,
    "abiflags": sysconfig.get_config_var("ABIFLAGS"),
    "interpreter": platform.python_implementation().lower(),
    "ext_suffix": ext_suffix,
    "soabi": sysconfig.get_config_var("SOABI") or None,
    "abi_tag": get_abi_tag(),
    "platform": sysconfig.get_platform(),
    # This one isn't technically necessary, but still very useful for sanity checks
    "system": platform.system().lower(),
    # This one is for generating a config file for pyo3
    "pointer_width": struct.calcsize("P") * 8,
}

print(json.dumps(metadata))
