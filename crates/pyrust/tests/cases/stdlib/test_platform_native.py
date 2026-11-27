# CPython 3.12 parity tests for the 100% native `platform` module
# (issue #3469).  Everything printed here is deterministic and identical on
# every host: host-dependent values are only compared against each other, and
# the platform-specific code paths are driven through monkeypatched module
# globals (exactly what CPython's own test-suite does).

import copy
import os
import pickle
import platform
import sys
import tempfile

TMP = tempfile.mkdtemp()


def write(name, data):
    path = os.path.join(TMP, name)
    with open(path, "wb") as f:
        f.write(data)
    return path


class Patch:
    """Temporarily replace attributes of the platform module."""

    def __init__(self, **values):
        self.values = values

    def __enter__(self):
        self.saved = {k: getattr(platform, k) for k in self.values}
        for k, v in self.values.items():
            setattr(platform, k, v)
        platform._platform_cache.clear()
        return self

    def __exit__(self, *exc):
        for k, v in self.saved.items():
            setattr(platform, k, v)
        platform._platform_cache.clear()
        return False


def show_error(label, fn, *args, **kwargs):
    try:
        print(label, "->", fn(*args, **kwargs))
    except Exception as e:
        print(label, "->", type(e).__name__, e)


# ── uname_result ─────────────────────────────────────────────────────────────
print("== uname_result ==")
UR = platform.uname_result
u = UR("Linux", "host", "6.8.0", "#1 SMP", "x86_64")
print("repr:", repr(u))
print("str:", str(u))
print("len:", len(u))
print("fields:", UR._fields)
print("module/name:", UR.__module__, UR.__name__)
print("mro:", [c.__name__ for c in UR.__mro__])
print("is tuple:", isinstance(u, tuple), "has base:", isinstance(u, UR.__mro__[1]))
print("attrs:", u.system, u.node, u.release, u.version, u.machine)
print("index:", u[0], u[4], u[-2], u[1:3], u[:2], u[::2][:2])
print("index 5 is processor:", u[5] == u.processor, u[-1] == u.processor)
print("index error:", end=" ")
try:
    u[6]
except IndexError as e:
    print("IndexError", e)
print("tuple len:", len(tuple(u)), len(list(u)), len(list(iter(u))))
print("tuple head:", tuple(u)[:5])
a, b, c, d, e, f = u
print("unpack 6:", (a, b, c, d, e) == tuple(u)[:5], f == u.processor)
print("eq 5:", u == ("Linux", "host", "6.8.0", "#1 SMP", "x86_64"))
print("eq 6:", u == tuple(u))
print("eq self:", u == UR("Linux", "host", "6.8.0", "#1 SMP", "x86_64"))
print("ne other:", u != UR("Linux", "host2", "6.8.0", "#1 SMP", "x86_64"))
print("asdict keys:", list(u._asdict().keys()))
print("asdict values:", list(u._asdict().values()) == list(u))
r = u._replace(system="Darwin")
print("replace:", repr(r), type(r).__name__, len(r))
print("replace unknown:", end=" ")
try:
    u._replace(bogus=1)
except ValueError as e:
    print("ValueError", e)
m = UR._make(["a", "b", "c", "d", "e"])
print("make:", repr(m), len(m), type(m).__name__)
m2 = UR._make(("a", "b", "c", "d", "e"))
print("make tuple:", m == m2)
for bad in (["only", "two"], list("abcdef"), []):
    try:
        UR._make(bad)
    except TypeError:
        print("make bad", len(bad), ": TypeError")
print("match_args:", UR.__match_args__)
print("getnewargs:", u.__getnewargs__())
print("reduce:", u.__reduce__()[0] is UR, u.__reduce__()[1])
p = pickle.loads(pickle.dumps(u))
print("pickle:", type(p).__name__, p == u, tuple(p) == tuple(u))
print("copy:", copy.copy(u) == u, type(copy.copy(u)).__name__)
print("deepcopy:", copy.deepcopy(u) == u)
print("hash equal:", hash(u) == hash(tuple(u)[:5]))
print("count/index:", u.count("host"), u.index("host"))
print("contains:", "host" in u, "nope" in u)
print("sorted:", sorted(u[:5]))
print("doc:", UR.__doc__)
print("instance dict before:", sorted(vars(u)))
proc = u.processor
print("processor str:", isinstance(proc, str))
print("instance dict after:", sorted(vars(u)))
print("cached:", u.processor is proc)
u.processor = "custom-cpu"
print("override:", u.processor, u[5], tuple(u)[5], len(u), repr(u))
print("pickle override:", pickle.loads(pickle.dumps(u)) == u)
print("classattr processor:", type(UR.processor).__name__)

# ── uname() and the accessors ────────────────────────────────────────────────
print("== uname ==")
platform._uname_cache = None
first = platform.uname()
print("cached object:", platform.uname() is first, platform._uname_cache is first)
print("type:", type(first).__name__, len(first), all(isinstance(x, str) for x in first))
print("no unknown:", all(x != "unknown" for x in first))
platform._uname_cache = None
second = platform.uname()
print("recomputed equal:", second == first, tuple(second) == tuple(first))
print("accessors:", platform.system() == first.system, platform.node() == first.node,
      platform.release() == first.release, platform.version() == first.version,
      platform.machine() == first.machine, platform.processor() == first.processor)
print("system non-empty:", bool(platform.system()), platform.system() == sys.platform.capitalize()
      or platform.system() in ("Windows", "Linux", "Darwin"))
print("unknown_as_blank:", repr(platform._unknown_as_blank("unknown")),
      repr(platform._unknown_as_blank("x")), platform._unknown_as_blank(3))
fake = UR("Fake", "n", "1", "v", "m")
with Patch(_uname_cache=fake):
    print("cache honoured:", platform.uname() is fake, platform.system(), platform.node(),
          platform.release(), platform.version(), platform.machine())
platform._uname_cache = None
print("restored:", platform.uname() == first)
for name in ("uname", "system", "node", "release", "version", "machine", "processor",
             "python_implementation", "python_version", "python_version_tuple",
             "python_branch", "python_revision", "python_build", "python_compiler",
             "win32_edition", "win32_is_iot", "freedesktop_os_release"):
    show_error("%s(1)" % name, getattr(platform, name), 1)

# ── system_alias ─────────────────────────────────────────────────────────────
print("== system_alias ==")
for args in (
    ("SunOS", "5.10", "Generic_147441-01"),
    ("SunOS", "5.6", "v"),
    ("SunOS", "4.1.3", "v"),
    ("SunOS", "5", "v"),
    ("SunOS", "x.y", "v"),
    ("SunOS", "6.0.1", "v"),
    ("win32", "10", "10.0.19041"),
    ("win16", "3.1", "x"),
    ("Windows", "10", "x"),
    ("Rhapsody", "5.6", "v"),
    ("IRIX", "6.5", "v"),
    ("IRIX64", "6.5", "v"),
    ("Linux", "6.8.0", "#1"),
    ("Darwin", "23.0.0", "v"),
    ("", "", ""),
):
    print(args[0] or "''", "->", platform.system_alias(*args))
print("kw:", platform.system_alias(system="SunOS", release="5.8", version="v"))
show_error("system_alias()", platform.system_alias)
show_error("system_alias(1)", platform.system_alias, 1)
show_error("system_alias SunOS int", platform.system_alias, "SunOS", 5, "v")
print("returns tuple:", type(platform.system_alias("Linux", "1", "2")).__name__)

# ── _platform helper ─────────────────────────────────────────────────────────
print("== _platform ==")
print(platform._platform("Linux", "6.8.0-1 (x)", "x86_64", "", "with", "glibc2.39"))
print(platform._platform("a b", "c/d", "e\\f", "g:h", "i;j", 'k"l', "(m)", "n"))
print(platform._platform("unknown", "x", "unknown"))
print(platform._platform("  pad  ", "x"))
print(platform._platform("a", "", "b"))
print(platform._platform("a--b", "--c--"))
print(platform._platform("a", "b-", "-"))
show_error("_platform()", platform._platform)
show_error("_platform('')", platform._platform, "")
show_error("_platform('unknown')", platform._platform, "unknown")
show_error("_platform(1)", platform._platform, 1)
show_error("_platform(x=1)", platform._platform, x=1)

# ── architecture ─────────────────────────────────────────────────────────────
print("== architecture ==")
arch = platform.architecture()
print("default:", type(arch).__name__, len(arch), arch)
print("default again:", platform.architecture(sys.executable) == arch)
print("custom empty exe:", platform.architecture(executable="", bits="32bit", linkage="custom"))
print("empty exe default bits:", platform.architecture(executable="")[0])
print("bits only:", platform.architecture(bits="16bit")[0])
print("linkage only:", platform.architecture(executable="", linkage="COFF"))
print("positional:", platform.architecture("", "48bit", "ELF"))
text = write("plain.txt", b"hello world\n")
print("text file:", platform.architecture(text))
print("text file custom:", platform.architecture(text, "7bit", "L"))
print("missing file:", platform.architecture(os.path.join(TMP, "missing"), "9bit", "X"))
print("pathlike:", platform.architecture(os.fsencode(text), "5bit", "Y"))
with Patch(**{}):
    pass
show_error("architecture(1,2,3,4)", platform.architecture, 1, 2, 3, 4)
show_error("architecture(bogus=1)", platform.architecture, bogus=1)

# ── platform() ───────────────────────────────────────────────────────────────
print("== platform() ==")
platform._platform_cache.clear()
full = platform.platform()
terse = platform.platform(terse=True)
aliased = platform.platform(aliased=True)
print("types:", type(full).__name__, type(terse).__name__, type(aliased).__name__)
print("bool keys:", platform.platform(0, 0) == full, platform.platform(False, False) == full)
print("cache keys:", sorted(platform._platform_cache.keys()))
print("cache hit:", platform.platform() is full)
print("truthy args:", platform.platform(2, "x") == platform.platform(True, True))
print("no spaces:", " " not in full, "/" not in full, ":" not in full)
print("starts:", full.startswith(platform.system()) or platform.system() == "Darwin")
print("poisoned cache:", end=" ")
platform._platform_cache[(False, False)] = "cached!"
print(platform.platform())
platform._platform_cache.clear()
print("fresh again:", platform.platform() == full)


def fake_uname(system, node, release, version, machine, processor):
    result = UR(system, node, release, version, machine)
    result.processor = processor
    return lambda: result


def libc(lib, ver):
    return lambda executable=None, lib_="", version_="", chunksize=16384: (lib, ver)


for label, kwargs in (
    ("linux glibc", dict(uname=fake_uname("Linux", "n", "6.8.0-1-generic", "#1 SMP", "x86_64", "x86_64"),
                         libc_ver=libc("glibc", "2.39"))),
    ("linux musl", dict(uname=fake_uname("Linux", "n", "5.15.0", "#1", "aarch64", "arm"),
                        libc_ver=libc("", ""))),
    ("linux libc only", dict(uname=fake_uname("Linux", "n", "5.15.0", "#1", "x86_64", ""),
                             libc_ver=libc("libc", ""))),
    ("windows", dict(uname=fake_uname("Windows", "n", "11", "10.0.22631", "AMD64", "Intel64 Family 6"),
                     win32_ver=lambda release="", version="", csd="", ptype="": ("11", "10.0.22631", "SP0", "Multiprocessor Free"))),
    ("darwin", dict(uname=fake_uname("Darwin", "n", "23.1.0", "Darwin Kernel", "arm64", "arm"),
                    mac_ver=lambda: ("14.1", ("", "", ""), "arm64"))),
    ("darwin no mac_ver", dict(uname=fake_uname("Darwin", "n", "23.1.0", "Darwin Kernel", "arm64", "arm"),
                               mac_ver=lambda: ("", ("", "", ""), ""),
                               architecture=lambda exe=None: ("64bit", ""))),
    ("sunos", dict(uname=fake_uname("SunOS", "n", "5.10", "Generic", "sun4u", "sparc"),
                   architecture=lambda exe=None: ("64bit", "ELF"))),
    ("java", dict(uname=fake_uname("Java", "n", "1.8", "OpenJDK", "x", "y"),
                  java_ver=lambda: ("1.8", "Oracle", ("VM", "25", "Oracle"), ("Linux", "5.4", "amd64")))),
    ("java no os", dict(uname=fake_uname("Java", "n", "1.8", "OpenJDK", "x", "y"),
                        java_ver=lambda: ("1.8", "Oracle", ("VM", "25", "Oracle"), ("", "", "")))),
    ("freebsd", dict(uname=fake_uname("FreeBSD", "n", "14.0-RELEASE", "v", "amd64", "amd64"),
                     architecture=lambda exe=None: ("64bit", "ELF"))),
    ("unknown parts", dict(uname=fake_uname("Plan9", "n", "unknown", "v", "unknown", "x"),
                           architecture=lambda exe=None: ("", ""))),
):
    with Patch(**kwargs):
        res = []
        for aliased_flag in (False, True):
            for terse_flag in (False, True):
                res.append(platform.platform(aliased_flag, terse_flag))
        print(label, "->", res)

with Patch(uname=fake_uname("Windows", "n", "11", "10.0.22631", "AMD64", "x"),
           win32_ver=lambda *a, **k: ("11", "10.0.22631", "SP1", "Multiprocessor Free")):
    print("win32_ver gets version:", end=" ")
    seen = []
    platform.win32_ver = lambda *a, **k: (seen.append((a, k)), ("11", "v", "SPX", "t"))[1]
    print(platform.platform(), seen)

with Patch(uname=lambda: ("A", "n", "1", "v", "m")):
    show_error("short uname", platform.platform)
with Patch(uname=lambda: ("A", "n", "1", "v", "m", "p", "q")):
    show_error("long uname", platform.platform)
with Patch(uname=lambda: ("SunOS", "n", "5.11", "v", "m", "p")):
    show_error("tuple uname aliased", platform.platform, True, True)

# ── python_* / _sys_version ──────────────────────────────────────────────────
print("== python_* ==")
CPY = "3.12.3 (main, Apr 15 2024, 17:48:16) [MSC v.1929 64 bit (AMD64)]"
for text in (
    CPY,
    "2.4.3 (#1, Jun 21 2006, 13:54:21) \n[GCC 3.3.4 (pre 3.3.5 20040809)]",
    "2.5.2 (r252:60911, Apr 21 2008, 11:12:42) \n[GCC 4.2.3]",
    "3.6.10 (default, Dec 10 2019, 10:20:20) [Clang 10.0.0 ]",
    "2.7 (r27:82500, Aug  7 2010, 17:30:11) \n[GCC 4.4.5]",
    "3.12.0b1 (tags/v3.12.0b1:abc, May 22 2023, 12:00:00) [GCC 12.2.0]",
    "3.9.1 (default, Dec 11 2020)  [GCC 9]",
    "3.9.1 (default)  [GCC 9]",
    "3.10.4+ (heads/3.10:abc, Jan  1 2022, 00:00:00) [GCC 11.2.0]",
    "2.7.1 (r271:86832, Feb  4 2011, 11:10:13) \n[PyPy 1.5.0-alpha0 with GCC 4.4]",
    "3.9.0 (7e0b2c3, Oct 10 2020, 12:00:00)\n[PyPy 7.3.3 with MSC v.1927 64 bit (AMD64)]",
):
    show_error(repr(text[:34]), platform._sys_version, text)
for text in ("", "garbage", "PyRust 0.2", "3.12.3 main", "3.12.3 (main) no brackets", "PyPy bad"):
    show_error("bad %r" % text, platform._sys_version, text)
print("cache entries:", CPY in platform._sys_version_cache, "garbage" in platform._sys_version_cache)
print("cache identity:", platform._sys_version(CPY) is platform._sys_version(CPY))
show_error("non-str", platform._sys_version, 5)
show_error("two args", platform._sys_version, CPY, 1)

fake_version = ("CPython", "3.12.3", "branch", "rev", "7", "Apr 15 2024 17:48:16", "GCC 13")
with Patch(_sys_version=lambda *a: fake_version):
    print("implementation:", platform.python_implementation())
    print("version:", platform.python_version())
    print("version_tuple:", platform.python_version_tuple())
    print("branch:", platform.python_branch())
    print("revision:", platform.python_revision())
    print("build:", platform.python_build())
    print("compiler:", platform.python_compiler())
with Patch(_sys_version=lambda *a: ("PyPy", "7.3.3", "", "", "x", "y", "")):
    print("pypy:", platform.python_implementation(), platform.python_version_tuple(),
          platform.python_build(), repr(platform.python_compiler()))

old_platform = sys.platform
sys.platform = "java1.8"
try:
    show_error("jython", platform._sys_version, "2.5.1 (Release_2_5_1:1, Jul 3 2009, 12:00:00) \n[Java HotSpot]")
    show_error("jython bad", platform._sys_version, "nonsense")
finally:
    sys.platform = old_platform

vt = platform.python_version_tuple()
print("host version tuple:", type(vt).__name__, len(vt), all(isinstance(x, str) for x in vt))
print("host version:", platform.python_version() == ".".join(vt),
      platform.python_version().count(".") == 2)
print("host impl:", platform.python_implementation() in ("CPython", "PyRust"))
print("host build:", type(platform.python_build()).__name__, len(platform.python_build()),
      all(isinstance(x, str) for x in platform.python_build()))
print("host strs:", isinstance(platform.python_branch(), str), isinstance(platform.python_revision(), str),
      isinstance(platform.python_compiler(), str))

# ── win32_ver / win32_edition / win32_is_iot ─────────────────────────────────
print("== win32 ==")
print("client table:", platform._WIN32_CLIENT_RELEASES[:2], len(platform._WIN32_CLIENT_RELEASES))
print("server table:", platform._WIN32_SERVER_RELEASES[:2], len(platform._WIN32_SERVER_RELEASES))
for version, is_client in (
    ("10.0.22621", True),
    ("10.0.22000", True),
    ("10.0.19045", True),
    ("10.1.0", True),
    ("6.3.9600", True),
    ("6.2.9200", True),
    ("6.1.7601", True),
    ("6.0.6002", True),
    ("5.2.3790", True),
    ("5.2.1", True),
    ("5.1.2600", True),
    ("5.0.2195", True),
    ("4.0.1381", True),
    ("10.0.20348", False),
    ("10.0.17763", False),
    ("10.0.14393", False),
    ("10.0.99999", False),
    ("10.5", False),
    ("6.3.9600", False),
    ("6.1.7601", False),
    ("5.2.3790", False),
    ("4.0", False),
):
    with Patch(_win32_ver=lambda v, c, p, _v=version, _c=is_client: (_v, "SP2", "Free", _c)):
        print(version, is_client, "->", platform.win32_ver())
with Patch(_win32_ver=lambda v, c, p: (v, c, p, True)):
    print("passthrough:", platform.win32_ver(release="R", version="", csd="C", ptype="P"))
    print("passthrough version:", platform.win32_ver("R", "10.0.22000", "C", "P"))
    print("positional:", platform.win32_ver("R", "", "C"))
with Patch(_win32_ver=lambda v, c, p: ("1.x.3", c, p, True)):
    show_error("bad version", platform.win32_ver)
with Patch(_win32_ver=lambda v, c, p: ("", "", "", False)):
    print("empty version keeps release:", platform.win32_ver(release="keep"))
real = platform.win32_ver()
print("real shape:", type(real).__name__, len(real), all(isinstance(x, str) for x in real))
if sys.platform != "win32":
    print("non-windows win32_ver:", real, platform.win32_ver("a", "", "c", "d"))
show_error("win32_ver(1,2,3,4,5)", platform.win32_ver, 1, 2, 3, 4, 5)
edition = platform.win32_edition()
print("edition:", edition is None or isinstance(edition, str))
if sys.platform != "win32":
    print("non-windows edition:", edition, platform.win32_is_iot())
for name in ("IoTUAP", "NanoServer", "WindowsCoreHeadless", "IoTEdgeOS", "Professional", "Core", "", None):
    with Patch(win32_edition=lambda _n=name: _n):
        print("is_iot", repr(name), platform.win32_is_iot())

# ── mac_ver / java_ver ───────────────────────────────────────────────────────
print("== mac_ver / java_ver ==")
with Patch(_mac_ver_xml=lambda: None):
    print("mac default:", platform.mac_ver())
    print("mac custom:", platform.mac_ver("10.5", ("a", "b", "c"), "ppc"))
    print("mac kw:", platform.mac_ver(release="1", machine="m"))
with Patch(_mac_ver_xml=lambda: ("14.1", ("", "", ""), "arm64")):
    print("mac xml:", platform.mac_ver("ignored"))
real_mac = platform.mac_ver()
print("mac real shape:", type(real_mac).__name__, len(real_mac), type(real_mac[1]).__name__, len(real_mac[1]))
if sys.platform != "darwin":
    print("non-mac:", real_mac, platform._mac_ver_xml())
print("java default:", platform.java_ver())
print("java custom:", platform.java_ver("1.8", "Oracle", ("a", "b", "c"), ("d", "e", "f")))
print("java kw:", platform.java_ver(vendor="V"))
show_error("java_ver(1,2,3,4,5)", platform.java_ver, 1, 2, 3, 4, 5)

# ── libc_ver ─────────────────────────────────────────────────────────────────
print("== libc_ver ==")
pad = b"\x00" * 50
cases = {
    "none": b"nothing here" + pad,
    "glibc1": b"x" + pad + b"GLIBC_2.17\x00" + pad,
    "glibc2": pad + b"GLIBC_2.2.5\x00GLIBC_2.34\x00GLIBC_2.17\x00" + pad,
    "glibc_order": pad + b"GLIBC_2.9\x00GLIBC_2.10\x00" + pad,
    "libc_init": pad + b"__libc_init" + pad,
    "so": pad + b"libc.so.6" + pad,
    "so_nover": pad + b"libc.so" + pad,
    "so_threads": pad + b"libc_r.so.1" + pad,
    "so_threads2": pad + b"libc_p.so.2.3" + pad,
    "so_two": pad + b"libc.so.5\x00libc.so.6.1\x00" + pad,
    "glibc_beats_so": pad + b"libc.so.6\x00GLIBC_2.31\x00libc.so.7" + pad,
    "so_then_glibc": pad + b"libc.so.6\x00GLIBC_2.31\x00" + pad,
    "init_then_glibc": pad + b"__libc_init\x00GLIBC_2.5\x00" + pad,
    "glibc_end": b"GLIBC_2.17",
    "libc_noso": pad + b"libc_foo bar libc " + pad,
    "glibc_nodigit": pad + b"GLIBC_ GLIBC_x" + pad,
    "empty": b"",
}
for name, data in cases.items():
    path = write("libc_" + name, data)
    print(name, "->", platform.libc_ver(path), platform.libc_ver(path, chunksize=16),
          platform.libc_ver(path, "L", "9.9"), platform.libc_ver(path, chunksize=7))
big = b"\x00" * 300 + b"GLIBC_2.31\x00" + b"\x00" * 3000 + b"GLIBC_2.39" + b"\x00" * 100
path = write("libc_big", big)
for chunk in (1, 64, 100, 1000, 4000, 16384):
    print("big chunk", chunk, platform.libc_ver(path, chunksize=chunk))
spanning = b"A" * 30 + b"GLIBC_2.35" + b"B" * 30
path = write("libc_span", spanning)
for chunk in (5, 31, 35, 40, 41, 45, 64):
    print("span chunk", chunk, platform.libc_ver(path, chunksize=chunk))
print("zero chunk:", platform.libc_ver(write("libc_z", b"GLIBC_2.1"), chunksize=0))
print("custom lib kept:", platform.libc_ver(write("libc_k", b"nothing"), "mylib", "1.0"))
print("path bytes:", platform.libc_ver(os.fsencode(write("libc_b", b"GLIBC_2.7"))))
try:
    platform.libc_ver(os.path.join(TMP, "nope"))
except OSError as e:
    print("missing:", type(e).__name__, e.errno is not None)
show_error("bad chunk", platform.libc_ver, write("libc_c", b"x"), "", "", "a")
host = platform.libc_ver()
print("host shape:", type(host).__name__, len(host), all(isinstance(x, str) for x in host))
if sys.platform.startswith("linux"):
    print("linux host:", host[0] in ("glibc", "libc", ""))
show_error("libc_ver(1,2,3,4,5)", platform.libc_ver, 1, 2, 3, 4, 5)

# ── _comparable_version / _norm_version / _syscmd_ver ───────────────────────
print("== version helpers ==")
for v in ("", "1", "2.17", "2.2.5", "1.0rc1", "1.0.dev3", "1.0-beta2", "3.0pl1", "1.0.alpha", "1.0.foo", "10.0.0.1+x", "007"):
    print(repr(v), platform._comparable_version(v))
print("ver_stages:", sorted(platform._ver_stages.items()))
for args in (("10.0.2195",), ("10.0.2195", "7"), ("5.00.2195",), ("1.2",), ("x.y.z",), ("1.2.3.4.5",), ("", ""), ("1_0.2",), (" 1.2 ",), ("1.-2",)):
    print("norm", args, "->", repr(platform._norm_version(*args)))
show_error("norm_version(1)", platform._norm_version, 1)
if sys.platform != "win32":
    print("syscmd_ver passthrough:", platform._syscmd_ver(), platform._syscmd_ver("a", "b", "c"))
print("syscmd_ver unsupported:", platform._syscmd_ver("S", "R", "V", supported_platforms=()))
print("syscmd_ver other:", platform._syscmd_ver("S", "R", "V", ("nonexistent-os",)))

# ── os-release ───────────────────────────────────────────────────────────────
print("== os-release ==")
lines = [
    "NAME=\"Ubuntu\"\n",
    "VERSION='22.04.3 LTS (Jammy Jellyfish)'\n",
    "ID=ubuntu\n",
    "ID_LIKE=debian\n",
    "# comment\n",
    "\n",
    "PRETTY_NAME=\"Ubuntu 22.04.3 LTS\"\n",
    "ESC=\"a\\\"b\\$c\\`d\\\\e\\'f\\x\"\n",
    "UNQUOTED=a\\\"b\n",
    "MIXED=\"abc'\n",
    "ONLYQUOTE=\"\n",
    "EMPTY=\n",
    "EMPTYQ=\"\"\n",
    "SPACED = x\n",
    "9NUM=1\n",
    "under_score=ok\n",
    "NOEOL=last",
    "TWO\nLINES=x\n",
    "CR=value\r\n",
    "NAME=Override\n",
    "=novalue\n",
    "noequals\n",
]
parsed = platform._parse_os_release(lines)
for key, value in parsed.items():
    print(repr(key), "=", repr(value))
print("defaults only:", platform._parse_os_release([]))
print("generator:", platform._parse_os_release(x for x in ["A=1\n", "ID=z\n"]))
show_error("non-iterable", platform._parse_os_release, 5)
show_error("non-str line", platform._parse_os_release, [1])
show_error("no args", platform._parse_os_release)

good = write("os-release", b'NAME="Test OS"\nID=testos\nVERSION_ID=\'1.2\'\nPRETTY_NAME="Test OS 1.2"\n')
missing1 = os.path.join(TMP, "missing1")
missing2 = os.path.join(TMP, "missing2")
saved_cache = platform._os_release_cache
platform._os_release_cache = None
with Patch(_os_release_candidates=(missing1, good, missing2)):
    info = platform.freedesktop_os_release()
    print("read:", info)
    print("cached:", platform._os_release_cache == info, platform._os_release_cache is not info)
    info["NAME"] = "mutated"
    print("copy semantics:", platform.freedesktop_os_release()["NAME"], platform._os_release_cache["NAME"])
platform._os_release_cache = None
with Patch(_os_release_candidates=(missing1, missing2)):
    try:
        platform.freedesktop_os_release()
    except OSError as e:
        print("missing:", type(e).__mro__[0] is not None, isinstance(e, OSError), e.args[1].replace(TMP, "<tmp>"))
        print("errno is int:", isinstance(e.errno, int))
    print("cache untouched:", platform._os_release_cache)
platform._os_release_cache = None
with Patch(_os_release_candidates=()):
    try:
        platform.freedesktop_os_release()
    except OSError as e:
        print("no candidates:", e.errno, e.args)
platform._os_release_cache = {"NAME": "preset"}
print("preset cache:", platform.freedesktop_os_release())
platform._os_release_cache = saved_cache
print("candidates:", platform._os_release_candidates)
if sys.platform.startswith("linux") and os.path.exists("/etc/os-release"):
    host_info = platform.freedesktop_os_release()
    print("linux host:", all(k in host_info for k in ("NAME", "ID", "PRETTY_NAME")))

# ── module surface ───────────────────────────────────────────────────────────
print("== module ==")
print("version:", platform.__version__)
print("no __all__:", not hasattr(platform, "__all__"))
for name in ("uname_result", "uname", "system", "node", "release", "version", "machine",
             "processor", "architecture", "platform", "system_alias", "win32_ver",
             "win32_edition", "win32_is_iot", "mac_ver", "java_ver", "libc_ver",
             "python_implementation", "python_version", "python_version_tuple",
             "python_branch", "python_revision", "python_build", "python_compiler",
             "freedesktop_os_release", "_sys_version", "_parse_os_release", "_platform",
             "_comparable_version", "_norm_version", "_syscmd_ver", "_node", "_win32_ver",
             "_mac_ver_xml", "_unknown_as_blank", "_ver_stages", "_uname_cache",
             "_platform_cache", "_sys_version_cache", "_os_release_cache",
             "_os_release_candidates", "_default_architecture",
             "_WIN32_CLIENT_RELEASES", "_WIN32_SERVER_RELEASES"):
    if not hasattr(platform, name):
        print("MISSING", name)
print("default arch table:", sorted(platform._default_architecture.items()))
print("no private leak:", not hasattr(platform, "_uname_result_processor"),
      not hasattr(platform, "_host_uname"))
print("imports:", platform.sys is sys, platform.os is os)
print("doc head:", platform.__doc__.strip().splitlines()[0])
print("node:", isinstance(platform._node(), str), platform._node("dflt") != "")
print("module name:", platform.uname_result.__module__, platform.platform.__name__,
      platform.uname.__name__)

import shutil

shutil.rmtree(TMP, ignore_errors=True)
