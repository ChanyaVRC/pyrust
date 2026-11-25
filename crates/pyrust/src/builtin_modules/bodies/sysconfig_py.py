"""Access to Python's configuration information."""

import os
from os.path import realpath
import platform as _platform
import re
import sys
import threading

__all__ = [
    "get_config_h_filename",
    "get_config_var",
    "get_config_vars",
    "get_makefile_filename",
    "get_path",
    "get_path_names",
    "get_paths",
    "get_platform",
    "get_python_version",
    "get_scheme_names",
    "parse_config_h",
]

# Keys for get_config_var() that are never converted to Python integers.
_ALWAYS_STR = {
    "MACOSX_DEPLOYMENT_TARGET",
}

_INSTALL_SCHEMES = {
    "posix_prefix": {
        "stdlib": "{installed_base}/{platlibdir}/python{py_version_short}",
        "platstdlib": "{platbase}/{platlibdir}/python{py_version_short}",
        "purelib": "{base}/lib/python{py_version_short}/site-packages",
        "platlib": "{platbase}/{platlibdir}/python{py_version_short}/site-packages",
        "include": "{installed_base}/include/python{py_version_short}{abiflags}",
        "platinclude": "{installed_platbase}/include/python{py_version_short}{abiflags}",
        "scripts": "{base}/bin",
        "data": "{base}",
    },
    "posix_home": {
        "stdlib": "{installed_base}/lib/python",
        "platstdlib": "{base}/lib/python",
        "purelib": "{base}/lib/python",
        "platlib": "{base}/lib/python",
        "include": "{installed_base}/include/python",
        "platinclude": "{installed_base}/include/python",
        "scripts": "{base}/bin",
        "data": "{base}",
    },
    "nt": {
        "stdlib": "{installed_base}/Lib",
        "platstdlib": "{base}/Lib",
        "purelib": "{base}/Lib/site-packages",
        "platlib": "{base}/Lib/site-packages",
        "include": "{installed_base}/Include",
        "platinclude": "{installed_base}/Include",
        "scripts": "{base}/Scripts",
        "data": "{base}",
    },
    "posix_venv": {
        "stdlib": "{installed_base}/{platlibdir}/python{py_version_short}",
        "platstdlib": "{platbase}/{platlibdir}/python{py_version_short}",
        "purelib": "{base}/lib/python{py_version_short}/site-packages",
        "platlib": "{platbase}/{platlibdir}/python{py_version_short}/site-packages",
        "include": "{installed_base}/include/python{py_version_short}{abiflags}",
        "platinclude": "{installed_platbase}/include/python{py_version_short}{abiflags}",
        "scripts": "{base}/bin",
        "data": "{base}",
    },
    "nt_venv": {
        "stdlib": "{installed_base}/Lib",
        "platstdlib": "{base}/Lib",
        "purelib": "{base}/Lib/site-packages",
        "platlib": "{base}/Lib/site-packages",
        "include": "{installed_base}/Include",
        "platinclude": "{installed_base}/Include",
        "scripts": "{base}/Scripts",
        "data": "{base}",
    },
}

if os.name == "nt":
    _INSTALL_SCHEMES["venv"] = _INSTALL_SCHEMES["nt_venv"]
else:
    _INSTALL_SCHEMES["venv"] = _INSTALL_SCHEMES["posix_venv"]


def _getuserbase():
    env_base = os.environ.get("PYTHONUSERBASE", None)
    if env_base:
        return env_base

    if sys.platform in {"emscripten", "vxworks", "wasi"}:
        return None

    def joinuser(*args):
        return os.path.expanduser(os.path.join(*args))

    if os.name == "nt":
        base = os.environ.get("APPDATA") or "~"
        return joinuser(base, "Python")

    framework = getattr(sys, "_framework", "")
    if sys.platform == "darwin" and framework:
        return joinuser(
            "~",
            "Library",
            framework,
            f"{sys.version_info[0]}.{sys.version_info[1]}",
        )

    return joinuser("~", ".local")


_HAS_USER_BASE = _getuserbase() is not None

if _HAS_USER_BASE:
    _INSTALL_SCHEMES.update(
        {
            "nt_user": {
                "stdlib": "{userbase}/Python{py_version_nodot_plat}",
                "platstdlib": "{userbase}/Python{py_version_nodot_plat}",
                "purelib": "{userbase}/Python{py_version_nodot_plat}/site-packages",
                "platlib": "{userbase}/Python{py_version_nodot_plat}/site-packages",
                "include": "{userbase}/Python{py_version_nodot_plat}/Include",
                "scripts": "{userbase}/Python{py_version_nodot_plat}/Scripts",
                "data": "{userbase}",
            },
            "posix_user": {
                "stdlib": "{userbase}/{platlibdir}/python{py_version_short}",
                "platstdlib": "{userbase}/{platlibdir}/python{py_version_short}",
                "purelib": "{userbase}/lib/python{py_version_short}/site-packages",
                "platlib": "{userbase}/lib/python{py_version_short}/site-packages",
                "include": "{userbase}/include/python{py_version_short}",
                "scripts": "{userbase}/bin",
                "data": "{userbase}",
            },
            "osx_framework_user": {
                "stdlib": "{userbase}/lib/python",
                "platstdlib": "{userbase}/lib/python",
                "purelib": "{userbase}/lib/python/site-packages",
                "platlib": "{userbase}/lib/python/site-packages",
                "include": "{userbase}/include/python{py_version_short}",
                "scripts": "{userbase}/bin",
                "data": "{userbase}",
            },
        }
    )

_SCHEME_KEYS = (
    "stdlib",
    "platstdlib",
    "purelib",
    "platlib",
    "include",
    "scripts",
    "data",
)

_PY_VERSION = (
    f"{sys.version_info[0]}.{sys.version_info[1]}.{sys.version_info[2]}"
)
_PY_VERSION_SHORT = f"{sys.version_info[0]}.{sys.version_info[1]}"
_PY_VERSION_SHORT_NO_DOT = f"{sys.version_info[0]}{sys.version_info[1]}"
_BASE_PREFIX = os.path.normpath(sys.base_prefix)
_BASE_EXEC_PREFIX = os.path.normpath(sys.base_exec_prefix)
_CONFIG_VARS_LOCK = threading.RLock()
_CONFIG_VARS = None
_CONFIG_VARS_INITIALIZED = False

_findvar1_rx = r"\$\(([A-Za-z][A-Za-z0-9_]*)\)"
_findvar2_rx = r"\${([A-Za-z][A-Za-z0-9_]*)}"


def _safe_realpath(path):
    try:
        return realpath(path)
    except OSError:
        return path


if sys.executable:
    _PROJECT_BASE = os.path.dirname(_safe_realpath(sys.executable))
else:
    _PROJECT_BASE = _safe_realpath(os.getcwd())

_sys_home = getattr(sys, "_home", None)
if _sys_home:
    _PROJECT_BASE = _sys_home

if "_PYTHON_PROJECT_BASE" in os.environ:
    _PROJECT_BASE = _safe_realpath(os.environ["_PYTHON_PROJECT_BASE"])


def is_python_build(check_home=None):
    if check_home is not None:
        import warnings

        warnings.warn(
            (
                "The check_home argument of sysconfig.is_python_build is "
                "deprecated and its value is ignored. "
                "It will be removed in Python 3.15."
            ),
            DeprecationWarning,
            stacklevel=2,
        )
    for fn in ("Setup", "Setup.local"):
        if os.path.isfile(os.path.join(_PROJECT_BASE, "Modules", fn)):
            return True
    return False


_PYTHON_BUILD = is_python_build()


def _subst_vars(s, local_vars):
    try:
        return s.format(**local_vars)
    except KeyError as var:
        try:
            return s.format(**os.environ)
        except KeyError:
            raise AttributeError(f"{var}") from None


def _extend_dict(target_dict, other_dict):
    target_keys = target_dict.keys()
    for key, value in other_dict.items():
        if key in target_keys:
            continue
        target_dict[key] = value


def _expand_vars(scheme, vars):
    res = {}
    if vars is None:
        vars = {}
    else:
        vars = dict(vars)
    _extend_dict(vars, get_config_vars())
    if os.name == "nt":
        vars = dict(vars)
        vars["platlibdir"] = "lib"

    for key, value in _INSTALL_SCHEMES[scheme].items():
        if os.name in ("posix", "nt"):
            value = os.path.expanduser(value)
        res[key] = os.path.normpath(_subst_vars(value, vars))
    return res


def _get_preferred_schemes():
    if os.name == "nt":
        return {
            "prefix": "nt",
            "home": "posix_home",
            "user": "nt_user",
        }
    framework = getattr(sys, "_framework", "")
    if sys.platform == "darwin" and framework:
        return {
            "prefix": "posix_prefix",
            "home": "posix_home",
            "user": "osx_framework_user",
        }
    return {
        "prefix": "posix_prefix",
        "home": "posix_home",
        "user": "posix_user",
    }


def get_preferred_scheme(key):
    if key == "prefix" and sys.prefix != sys.base_prefix:
        return "venv"
    scheme = _get_preferred_schemes()[key]
    if scheme not in _INSTALL_SCHEMES:
        raise ValueError(
            f"{key!r} returned {scheme!r}, which is not a valid scheme "
            f"on this platform"
        )
    return scheme


def get_default_scheme():
    return get_preferred_scheme("prefix")


def get_makefile_filename():
    """Return the path of the Makefile."""
    if _PYTHON_BUILD:
        return os.path.join(_PROJECT_BASE, "Makefile")
    if hasattr(sys, "abiflags"):
        config_dir_name = f"config-{_PY_VERSION_SHORT}{sys.abiflags}"
    else:
        config_dir_name = "config"
    multiarch = getattr(sys.implementation, "_multiarch", None)
    if multiarch:
        config_dir_name += f"-{multiarch}"
    return os.path.join(get_path("stdlib"), config_dir_name, "Makefile")


def parse_config_h(fp, vars=None):
    """Parse a config.h-style file."""
    if vars is None:
        vars = {}
    define_rx = re.compile(r"#define ([A-Z][A-Za-z0-9_]+) (.*)\n")
    undef_rx = re.compile(r"/\* #undef ([A-Z][A-Za-z0-9_]+) \*/\n")

    while True:
        line = fp.readline()
        if not line:
            break
        m = define_rx.match(line)
        if m:
            n, v = m.group(1, 2)
            try:
                if n in _ALWAYS_STR:
                    raise ValueError
                v = int(v)
            except ValueError:
                pass
            vars[n] = v
        else:
            m = undef_rx.match(line)
            if m:
                vars[m.group(1)] = 0
    return vars


def get_config_h_filename():
    """Return the path of pyconfig.h."""
    if _PYTHON_BUILD:
        if os.name == "nt":
            inc_dir = os.path.join(_PROJECT_BASE, "PC")
        else:
            inc_dir = _PROJECT_BASE
    else:
        inc_dir = get_path("platinclude")
    return os.path.join(inc_dir, "pyconfig.h")


def get_scheme_names():
    """Return a tuple containing the scheme names."""
    return tuple(sorted(_INSTALL_SCHEMES))


def get_path_names():
    """Return a tuple containing the path names."""
    return _SCHEME_KEYS


def get_paths(scheme=get_default_scheme(), vars=None, expand=True):
    """Return a mapping containing an install scheme."""
    if expand:
        return _expand_vars(scheme, vars)
    return _INSTALL_SCHEMES[scheme]


def get_path(name, scheme=get_default_scheme(), vars=None, expand=True):
    """Return a path corresponding to the scheme."""
    return get_paths(scheme, vars, expand)[name]


def _win_nodot_plat():
    _, arch = _host_os_arch()
    if arch == "x86":
        return _PY_VERSION_SHORT_NO_DOT + "-32"
    if arch == "aarch64":
        return _PY_VERSION_SHORT_NO_DOT + "-arm64"
    if arch == "arm":
        return _PY_VERSION_SHORT_NO_DOT + "-arm32"
    return _PY_VERSION_SHORT_NO_DOT


def _init_config_vars():
    global _CONFIG_VARS, _CONFIG_VARS_INITIALIZED
    _CONFIG_VARS = {}
    prefix = os.path.normpath(sys.prefix)
    exec_prefix = os.path.normpath(sys.exec_prefix)
    base_prefix = os.path.normpath(sys.base_prefix)
    base_exec_prefix = os.path.normpath(sys.base_exec_prefix)

    _CONFIG_VARS["prefix"] = prefix
    _CONFIG_VARS["exec_prefix"] = exec_prefix
    _CONFIG_VARS["py_version"] = _PY_VERSION
    _CONFIG_VARS["py_version_short"] = _PY_VERSION_SHORT
    _CONFIG_VARS["py_version_nodot"] = _PY_VERSION_SHORT_NO_DOT
    _CONFIG_VARS["installed_base"] = base_prefix
    _CONFIG_VARS["base"] = prefix
    _CONFIG_VARS["installed_platbase"] = base_exec_prefix
    _CONFIG_VARS["platbase"] = exec_prefix
    _CONFIG_VARS["projectbase"] = _PROJECT_BASE
    _CONFIG_VARS["platlibdir"] = sys.platlibdir
    _CONFIG_VARS["abiflags"] = getattr(sys, "abiflags", "")
    if os.name == "nt":
        winver = getattr(sys, "winver", None)
        _CONFIG_VARS["py_version_nodot_plat"] = (
            winver.replace(".", "") if isinstance(winver, str) else _win_nodot_plat()
        )
    else:
        _CONFIG_VARS["py_version_nodot_plat"] = ""

    if _HAS_USER_BASE:
        _CONFIG_VARS["userbase"] = _getuserbase()

    _CONFIG_VARS["LIBDEST"] = get_path("stdlib")
    _CONFIG_VARS["BINLIBDEST"] = get_path("platstdlib")
    _CONFIG_VARS["INCLUDEPY"] = get_path("include")
    _CONFIG_VARS["BINDIR"] = os.path.dirname(_safe_realpath(sys.executable)) if sys.executable else _PROJECT_BASE
    _CONFIG_VARS["TZPATH"] = ""

    if os.name == "nt":
        _CONFIG_VARS["EXT_SUFFIX"] = ".pyd"
        _CONFIG_VARS["EXE"] = ".exe"
        _CONFIG_VARS["VERSION"] = _PY_VERSION_SHORT_NO_DOT
        _CONFIG_VARS["VPATH"] = getattr(sys, "_vpath", "")
    else:
        _CONFIG_VARS["EXT_SUFFIX"] = ".so"
        _CONFIG_VARS["SHLIB_SUFFIX"] = ".so"
        _CONFIG_VARS["SOABI"] = getattr(sys.implementation, "cache_tag", "cpython-312")
        _CONFIG_VARS["EXE"] = ""
        _CONFIG_VARS["VERSION"] = _PY_VERSION_SHORT

    srcdir = _CONFIG_VARS.get("srcdir", _PROJECT_BASE)
    if os.name == "posix":
        srcdir = os.path.dirname(get_makefile_filename())
    _CONFIG_VARS["srcdir"] = _safe_realpath(srcdir)
    _CONFIG_VARS_INITIALIZED = True


def get_config_vars(*args):
    """With no arguments, return a dictionary of all configuration
    variables relevant for the current platform.

    With arguments, return a list of values that result from looking up
    each argument in the configuration variable dictionary.
    """
    global _CONFIG_VARS_INITIALIZED

    if not _CONFIG_VARS_INITIALIZED:
        with _CONFIG_VARS_LOCK:
            if _CONFIG_VARS is None:
                _init_config_vars()
    else:
        if _CONFIG_VARS["base"] != os.path.normpath(sys.prefix):
            with _CONFIG_VARS_LOCK:
                _CONFIG_VARS_INITIALIZED = False
                _init_config_vars()

    if args:
        return [_CONFIG_VARS.get(name) for name in args]
    return _CONFIG_VARS


def get_config_var(name):
    """Return the value of a single variable from get_config_vars()."""
    return get_config_vars().get(name)


def get_platform():
    """Return a string that identifies the current platform."""
    host_os, host_arch = _host_os_arch()
    if os.name == "nt":
        if host_arch == "x86_64":
            return "win-amd64"
        if host_arch == "aarch64":
            return "win-arm64"
        if host_arch == "arm":
            return "win-arm32"
        return sys.platform

    if os.name != "posix":
        return sys.platform

    if "_PYTHON_HOST_PLATFORM" in os.environ:
        return os.environ["_PYTHON_HOST_PLATFORM"]

    u = _platform.uname()
    osname = (u.system or host_os).lower().replace("/", "")
    release = u.release or ""
    machine = (u.machine or host_arch).replace(" ", "_").replace("/", "-")

    if osname[:5] == "linux":
        return f"{osname}-{machine}"
    if osname[:6] == "darwin":
        # Convert Darwin kernel major version to macOS release if available.
        major_str = release.split(".", 1)[0]
        try:
            major = int(major_str)
        except ValueError:
            macos_ver = release
        else:
            if major >= 20:
                macos_ver = f"{major - 9}.0"
            elif major >= 4:
                macos_ver = f"10.{major - 4}"
            else:
                macos_ver = release
        return f"macosx-{macos_ver}-{machine}"
    if osname[:6] == "cygwin":
        osname = "cygwin"
        m = re.match(r"[\d.]+", release)
        if m:
            release = m.group()
    if release:
        return f"{osname}-{release}-{machine}"
    return f"{osname}-{machine}"


def get_python_version():
    """Return a string containing the major and minor Python version."""
    return _PY_VERSION_SHORT


def expand_makefile_vars(s, vars):
    """Expand Makefile-style variables -- '${foo}' or '$(foo)' -- in 's'."""
    while True:
        m = re.search(_findvar1_rx, s) or re.search(_findvar2_rx, s)
        if m:
            beg, end = m.span()
            s = s[0:beg] + vars.get(m.group(1), "") + s[end:]
        else:
            break
    return s
