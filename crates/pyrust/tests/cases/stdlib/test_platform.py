# CPython 3.12 parity tests for the `platform` module (issue #3209).

import platform
import sys

# ── uname() and uname_result ─────────────────────────────────────────────────
u = platform.uname()
print("isinstance uname_result:", isinstance(u, platform.uname_result))
print("isinstance tuple:", isinstance(u, tuple))
print("len(u):", len(u))
print("fields:", platform.uname_result._fields)
print("tuple len:", len(tuple(u)))
print(
    "tuple matches attrs:",
    tuple(u) == (u.system, u.node, u.release, u.version, u.machine, u.processor),
)
print(
    "index matches attrs:",
    [u[i] for i in range(6)]
    == [u.system, u.node, u.release, u.version, u.machine, u.processor],
)
print("slice:", u[:2] == (u.system, u.node), u[4:] == (u.machine, u.processor))
print("eq 5-tuple:", u == (u.system, u.node, u.release, u.version, u.machine))
print("eq 6-tuple:", u == tuple(u))
print("asdict keys:", list(u._asdict().keys()))
print("asdict values:", list(u._asdict().values()) == list(u))

# Manual uname_result construction & _make
u2 = platform.uname_result("Linux", "myhost", "6.8.0", "#1 SMP", "x86_64")
print("u2 repr:", repr(u2))
print("u2 len:", len(u2))
print("u2 first 5:", u2[:5])
u3 = platform.uname_result._make(["Darwin", "mac", "23.0", "ver", "arm64"])
print("u3 repr:", repr(u3))
try:
    platform.uname_result._make(["only", "two"])
except TypeError:
    print("_make bad len: TypeError")

# ── Individual accessors agree with uname() and OS ───────────────────────────
print("system matches:", platform.system() == u.system, platform.system())
print("node matches:", platform.node() == u.node, platform.node())
print("release matches:", platform.release() == u.release, bool(platform.release()))
print("version matches:", platform.version() == u.version, platform.version())
print("machine matches:", platform.machine() == u.machine, platform.machine())
print("processor matches:", platform.processor() == u.processor, platform.processor())

if sys.platform != "win32":
    print("posix release:", platform.release())

# ── architecture() ───────────────────────────────────────────────────────────
arch = platform.architecture()
print("architecture:", arch)
print(
    "architecture custom empty exe:",
    platform.architecture(executable="", bits="32bit", linkage="custom"),
)
print("architecture preset bits:", platform.architecture(bits="32bit"))

# ── platform() and system_alias() ────────────────────────────────────────────
p_full = platform.platform()
p_terse = platform.platform(terse=True)
p_aliased = platform.platform(aliased=True)
print("platform starts with system:", p_full.startswith(platform.system() + "-"))
print("terse starts with system:", p_terse.startswith(platform.system() + "-"))
print("terse <= full:", len(p_terse) <= len(p_full))
print("aliased non-empty:", isinstance(p_aliased, str) and len(p_aliased) > 0)
print("system_alias SunOS:", platform.system_alias("SunOS", "5.10", "Generic"))
print("system_alias win32:", platform.system_alias("win32", "10", "10.0"))

# ── Python runtime metadata ──────────────────────────────────────────────────
impl = platform.python_implementation()
ver = platform.python_version()
vtup = platform.python_version_tuple()
branch = platform.python_branch()
rev = platform.python_revision()
build = platform.python_build()
comp = platform.python_compiler()

print("impl:", impl in ("CPython", "PyRust"))
print(
    "version_tuple matches sys.version_info:",
    vtup
    == (
        str(sys.version_info.major),
        str(sys.version_info.minor),
        str(sys.version_info.micro),
    ),
    "len:",
    len(vtup),
    "major>=3 minor>=12:",
    int(vtup[0]) == 3 and int(vtup[1]) >= 12,
)
print("version_tuple digits:", all(isinstance(x, str) and x.isdigit() for x in vtup))
print("version matches tuple:", ver == ".".join(vtup))
print("branch str:", isinstance(branch, str))
print("revision str:", isinstance(rev, str))
print(
    "build pair:",
    isinstance(build, tuple)
    and len(build) == 2
    and all(isinstance(x, str) for x in build),
)
print("compiler non-empty str:", isinstance(comp, str) and len(comp) > 0)
