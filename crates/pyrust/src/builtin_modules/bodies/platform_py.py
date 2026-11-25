"""Access to underlying platform's identifying data.

CPython 3.12-compatible implementation of Python's ``platform`` module for
PyRust (issue #3209), backed by native OS probes in ``platform.rs``.
"""

import collections
import functools
import itertools
import struct
import sys

__all__ = [
    "uname_result",
    "uname",
    "system",
    "node",
    "release",
    "version",
    "machine",
    "processor",
    "architecture",
    "platform",
    "system_alias",
    "win32_ver",
    "libc_ver",
    "python_implementation",
    "python_version",
    "python_version_tuple",
    "python_branch",
    "python_revision",
    "python_build",
    "python_compiler",
]


def _unknown_as_blank(val):
    return "" if val == "unknown" else val


class uname_result(
    collections.namedtuple(
        "uname_result_base",
        "system node release version machine",
    )
):
    """A uname_result that's largely compatible with a simple namedtuple
    except that 'processor' is resolved late and cached.
    """

    _fields = ("system", "node", "release", "version", "machine", "processor")

    @functools.cached_property
    def processor(self):
        return _unknown_as_blank(_host_processor())

    def __iter__(self):
        return itertools.chain(super().__iter__(), (self.processor,))

    @classmethod
    def _make(cls, iterable):
        num_fields = len(cls._fields) - 1
        result = cls.__new__(cls, *iterable)
        if len(result) != num_fields + 1:
            msg = f"Expected {num_fields} arguments, got {len(result)}"
            raise TypeError(msg)
        return result

    def __getitem__(self, key):
        return tuple(self)[key]

    def __len__(self):
        return len(tuple(iter(self)))

    def __repr__(self):
        return (
            f"{self.__class__.__name__}("
            f"system={self.system!r}, node={self.node!r}, "
            f"release={self.release!r}, version={self.version!r}, "
            f"machine={self.machine!r})"
        )

    def __reduce__(self):
        return uname_result, tuple(self)[: len(self._fields) - 1]


_uname_cache = None


def uname():
    """Fairly portable uname interface. Returns a uname_result
    (system, node, release, version, machine, processor).
    """
    global _uname_cache
    if _uname_cache is not None:
        return _uname_cache
    vals = tuple(_unknown_as_blank(v) for v in _host_uname())
    _uname_cache = uname_result(*vals)
    return _uname_cache


def system():
    """Returns the system/OS name, e.g. 'Linux', 'Windows', or 'Darwin'."""
    return uname().system


def node():
    """Returns the computer's network name (which may not be fully qualified)."""
    return uname().node


def release():
    """Returns the system's release, e.g. '6.8.0' or '11'."""
    return uname().release


def version():
    """Returns the system's release version."""
    return uname().version


def machine():
    """Returns the machine type, e.g. 'x86_64' or 'AMD64'."""
    return uname().machine


def processor():
    """Returns the (true) processor name."""
    return uname().processor


_default_architecture = {
    "win32": ("", "WindowsPE"),
    "win16": ("", "Windows"),
    "dos": ("", "MSDOS"),
}


def architecture(executable=sys.executable, bits="", linkage=""):
    """Queries the given executable for bit architecture and linkage format."""
    if not bits:
        size = struct.calcsize("P")
        bits = str(size * 8) + "bit"

    if sys.platform in _default_architecture:
        if executable == sys.executable:
            b, l = _default_architecture[sys.platform]
            if b:
                bits = b
            if l:
                linkage = l
        return bits, linkage

    if executable:
        probed_bits, probed_linkage = _host_probe_executable(executable)
        if probed_bits:
            bits = probed_bits
        if probed_linkage:
            linkage = probed_linkage

    return bits, linkage


def win32_ver(release="", version="", csd="", ptype=""):
    """Get additional version information from the Windows Registry/API."""
    if sys.platform != "win32":
        return release, version, csd, ptype
    h_rel, h_ver, h_csd, h_ptype = _host_win32_ver()
    return (
        h_rel or release,
        h_ver or version,
        h_csd or csd,
        h_ptype or ptype,
    )


def libc_ver(executable=sys.executable, lib="", version="", chunksize=16384):
    """Determine the libc version linked against the executable."""
    if executable == sys.executable:
        h_lib, h_ver = _host_libc_ver()
        if h_lib:
            lib = h_lib
        if h_ver:
            version = h_ver
    return lib, version


def system_alias(system, release, version):
    """Returns (system, release, version) aliased to common marketing names."""
    if system == "SunOS":
        if release < "5":
            return system, release, version
        l = release.split(".")
        if l:
            try:
                major = int(l[0])
            except ValueError:
                pass
            else:
                major = major - 3
                l[0] = str(major)
                release = ".".join(l)
        if release < "6":
            system = "Solaris"
        else:
            system = "Solaris"
    elif system in ("win32", "win16"):
        system = "Windows"
    return system, release, version


def _platform(*args):
    """Helper to format the platform string in a filename-compatible format."""
    plat = "-".join(x.strip() for x in args if len(x))
    for old, new in (
        (" ", "_"),
        ("/", "-"),
        ("\\", "-"),
        (":", "-"),
        (";", "-"),
        ('"', "-"),
        ("(", "-"),
        (")", "-"),
        ("unknown", ""),
    ):
        plat = plat.replace(old, new)
    while "--" in plat:
        plat = plat.replace("--", "-")
    while plat and plat[-1] == "-":
        plat = plat[:-1]
    return plat


_platform_cache = {}


def platform(aliased=0, terse=0):
    """Returns a single string identifying the underlying platform."""
    aliased = bool(aliased)
    terse = bool(terse)
    result = _platform_cache.get((aliased, terse), None)
    if result is not None:
        return result

    sys_name, _node, rel, ver, mach, proc = uname()
    if mach == proc:
        proc = ""
    if aliased:
        sys_name, rel, ver = system_alias(sys_name, rel, ver)

    if sys_name == "Windows":
        _r, _v, csd, _p = win32_ver(version=ver)
        if terse:
            plat = _platform(sys_name, rel)
        else:
            plat = _platform(sys_name, rel, ver, csd)
    elif sys_name == "Linux":
        if terse:
            plat = _platform(sys_name, rel)
        else:
            libcname, libcversion = libc_ver()
            if libcname or libcversion:
                plat = _platform(sys_name, rel, mach, proc, "with", libcname + libcversion)
            else:
                plat = _platform(sys_name, rel, mach, proc)
    elif terse:
        plat = _platform(sys_name, rel)
    else:
        bits, linkage = architecture(sys.executable)
        plat = _platform(sys_name, rel, mach, proc, bits, linkage)

    _platform_cache[(aliased, terse)] = plat
    return plat


def python_implementation():
    """Returns a string identifying the Python implementation."""
    impl = getattr(sys.implementation, "name", "cpython")
    if impl == "cpython":
        return "CPython"
    if impl == "pypy":
        return "PyPy"
    return impl


def python_version():
    """Returns the Python version as string 'major.minor.patchlevel'."""
    vi = sys.version_info
    return f"{vi.major}.{vi.minor}.{vi.micro}"


def python_version_tuple():
    """Returns the Python version as tuple (major, minor, patchlevel) of strings."""
    vi = sys.version_info
    return (str(vi.major), str(vi.minor), str(vi.micro))


def python_branch():
    """Returns a string identifying the Python implementation SCM branch."""
    return ""


def python_revision():
    """Returns a string identifying the Python implementation SCM revision."""
    return ""


def python_build():
    """Returns a tuple (buildno, builddate) stating the Python build number and date."""
    return ("main", "")


def python_compiler():
    """Returns a string identifying the compiler used for compiling Python."""
    return _host_compiler()
