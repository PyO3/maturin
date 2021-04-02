import json
import platform
import sys
import sysconfig

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
    "major": sys.version_info.major,
    "minor": sys.version_info.minor,
    "abiflags": sysconfig.get_config_var("ABIFLAGS"),
    "interpreter": platform.python_implementation().lower(),
    "ext_suffix": ext_suffix,
    "abi_tag": (sysconfig.get_config_var("SOABI") or "-").split("-")[1] or None,
    # This one isn't technically necessary, but still very useful for sanity checks
    "platform": platform.system().lower(),
    # We need this one for windows abi3 builds
    "base_prefix": sys.base_prefix,
}

print(json.dumps(metadata))
