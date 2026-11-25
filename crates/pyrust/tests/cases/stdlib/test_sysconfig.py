# CPython 3.12 parity tests for the `sysconfig` module (issue #3229).

import io
import os
import sys
import sysconfig

# 1. Module metadata and __all__
print("in sys.modules:", "sysconfig" in sys.modules)
print("__all__:", sysconfig.__all__)
for name in sysconfig.__all__:
    assert hasattr(sysconfig, name), f"missing {name}"
for extra in ("get_default_scheme", "get_preferred_scheme", "is_python_build"):
    assert hasattr(sysconfig, extra), f"missing {extra}"

# 2. Version and platform
print("python_version:", sysconfig.get_python_version())
plat = sysconfig.get_platform()
print("platform type/nonempty:", isinstance(plat, str), len(plat) > 0)
if sys.platform == "win32":
    print("platform matches:", plat)
elif sys.platform == "linux":
    print("platform matches:", plat)
elif sys.platform == "darwin":
    print("platform darwin:", plat.startswith("macosx-"))

# 3. Schemes and path names
schemes = sysconfig.get_scheme_names()
print("scheme_names sorted tuple:", isinstance(schemes, tuple), list(schemes) == sorted(schemes))
required_schemes = {
    "nt",
    "nt_user",
    "nt_venv",
    "osx_framework_user",
    "posix_home",
    "posix_prefix",
    "posix_user",
    "posix_venv",
    "venv",
}
print("required schemes present:", required_schemes.issubset(set(schemes)))

path_names = sysconfig.get_path_names()
print("path_names:", path_names)

default_scheme = sysconfig.get_default_scheme()
print("default_scheme in schemes:", default_scheme in schemes)
print(
    "preferred schemes valid:",
    sysconfig.get_preferred_scheme("prefix") in schemes,
    sysconfig.get_preferred_scheme("home") == "posix_home",
    sysconfig.get_preferred_scheme("user") in schemes,
    sysconfig.get_preferred_scheme("prefix") == default_scheme,
)
try:
    sysconfig.get_preferred_scheme("invalid_key_3229")
except KeyError as e:
    print("preferred invalid:", type(e).__name__, str(e))

# 4. get_paths / get_path (unexpanded and expanded with deterministic vars)
for scheme in ("posix_prefix", "posix_home", "nt"):
    raw_paths = sysconfig.get_paths(scheme, expand=False)
    print("unexpanded", scheme, "scripts:", raw_paths["scripts"])
    print("unexpanded", scheme, "purelib:", raw_paths["purelib"])
    assert sysconfig.get_path("scripts", scheme, expand=False) == raw_paths["scripts"]

custom_vars = {
    "base": "/custom/base",
    "installed_base": "/custom/inst",
    "platbase": "/custom/plat",
    "installed_platbase": "/custom/inst_plat",
    "platlibdir": "lib",
    "py_version_short": "3.12",
    "py_version_nodot": "312",
    "py_version_nodot_plat": "312",
    "abiflags": "",
    "userbase": "/custom/user",
}
for scheme in ("posix_prefix", "posix_home", "nt", "posix_user", "nt_user"):
    exp = sysconfig.get_paths(scheme, vars=custom_vars, expand=True)
    print(
        "expanded",
        scheme,
        [(k, exp[k]) for k in sorted(exp.keys())],
    )
    for k in path_names:
        assert sysconfig.get_path(k, scheme, vars=custom_vars, expand=True) == exp[k]

# Default get_paths() structure check (without asserting host-specific prefix)
default_paths = sysconfig.get_paths()
print(
    "default_paths valid:",
    isinstance(default_paths, dict),
    set(path_names).issubset(set(default_paths.keys())),
    all(isinstance(v, str) and len(v) > 0 for v in default_paths.values()),
    all(sysconfig.get_path(k) == default_paths[k] for k in path_names),
)

# Error handling on unknown scheme / path name
try:
    sysconfig.get_paths("no_such_scheme_3229")
except KeyError:
    print("unknown scheme: KeyError")
try:
    sysconfig.get_path("no_such_path_3229")
except KeyError:
    print("unknown path: KeyError")

# 5. get_config_vars and get_config_var
cfg = sysconfig.get_config_vars()
print("config_vars dict:", isinstance(cfg, dict))
for required_key in (
    "py_version_short",
    "py_version_nodot",
    "base",
    "platbase",
    "installed_base",
    "prefix",
    "exec_prefix",
    "platlibdir",
    "userbase",
):
    assert required_key in cfg, f"missing config var {required_key}"
print("config_var short:", sysconfig.get_config_var("py_version_short"))
print("config_var nodot:", sysconfig.get_config_var("py_version_nodot"))
print("config_var missing:", sysconfig.get_config_var("NONEXISTENT_VAR_3229"))
print(
    "config_vars multi:",
    sysconfig.get_config_vars(
        "py_version_short", "py_version_nodot", "NONEXISTENT_VAR_3229"
    ),
)

# 6. is_python_build, parse_config_h, config/makefile filenames
print("is_python_build:", isinstance(sysconfig.is_python_build(), bool), sysconfig.is_python_build())
sample_h = io.StringIO(
    "#define HAVE_STDIO_H 1\n"
    '#define PY_FORMAT_SIZE_T "z"\n'
    "/* #undef HAVE_FOO_H */\n"
    "/* ignored comment */\n"
)
parsed_h = sysconfig.parse_config_h(sample_h)
print("parse_config_h:", sorted(parsed_h.items()))

cfg_h = sysconfig.get_config_h_filename()
mk_f = sysconfig.get_makefile_filename()
print(
    "filenames:",
    isinstance(cfg_h, str) and os.path.basename(cfg_h) == "pyconfig.h",
    isinstance(mk_f, str) and os.path.basename(mk_f) == "Makefile",
)
