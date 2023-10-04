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
    "platform": sysconfig.get_platform(),
    # This one isn't technically necessary, but still very useful for sanity checks
    "system": platform.system().lower(),
    # This one is for generating a config file for pyo3
    "pointer_width": struct.calcsize("P") * 8,
}

print(json.dumps(metadata))
