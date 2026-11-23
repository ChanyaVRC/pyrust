# Issue #2970:
#   1. Built-in module `__dict__` / `vars(m)` returns the module's one live
#      namespace dictionary rather than a fresh snapshot.
#   2. `vars(sys)` exposes expanded, portable CPython 3.12 runtime members
#      while keeping the five-slot dunder prefix.
#   3. `types.ModuleType` is `type(sys)` and is constructible and subclassable.

import builtins
import json
import math
import os
import sys
import types

SLOTS = ["__name__", "__doc__", "__package__", "__loader__", "__spec__"]

# ── 1. Live built-in module __dict__ / vars() ────────────────────────────────

for mod in (math, json, os, sys, builtins):
    d1 = mod.__dict__
    d2 = vars(mod)
    d3 = mod.__dict__
    print(mod.__name__, d1 is d2, d1 is d3, list(d1)[:5] == SLOTS)

# Bidirectional mutation through __dict__ and attribute access on a builtin
# module that starts in compact storage (`json` and `math`).
math.__dict__["_probe_2970_a"] = 42
print(math._probe_2970_a == 42)
math._probe_2970_b = 99
print(math.__dict__["_probe_2970_b"] == 99)
del math.__dict__["_probe_2970_a"]
print(hasattr(math, "_probe_2970_a"))
del math._probe_2970_b
print("_probe_2970_b" in math.__dict__)

# Mapping methods mutate the live namespace.
math.__dict__.update({"_probe_2970_u1": "one", "_probe_2970_u2": "two"})
print(math._probe_2970_u1, math._probe_2970_u2)
print(math.__dict__.setdefault("_probe_2970_u3", "three"), math._probe_2970_u3)
print(math.__dict__.pop("_probe_2970_u1"), hasattr(math, "_probe_2970_u1"))
math.__dict__.pop("_probe_2970_u2")
math.__dict__.pop("_probe_2970_u3")

# Mutating `builtins.__dict__` is immediately visible to global name lookups
# (verifying module cache invalidation once `__dict__` is exposed).
builtins.__dict__["_probe_2970_builtin"] = "from_builtins_dict"
print(_probe_2970_builtin)
del builtins.__dict__["_probe_2970_builtin"]
try:
    _ = _probe_2970_builtin
    print("unexpected")
except NameError:
    print("builtins_del_ok")

# `sys.modules[name]` shares the exact same live dict.
print(sys.modules["math"].__dict__ is math.__dict__)

# `__dict__` attribute itself remains read-only.
for op in ("set", "del"):
    try:
        if op == "set":
            math.__dict__ = {}
        else:
            del math.__dict__
    except AttributeError as exc:
        print(f"readonly_{op}", str(exc))

# ── 2. Expanded vars(sys) surface ────────────────────────────────────────────

print("sys_head5", list(vars(sys))[:5] == SLOTS)
print("sys_surface_expanded", len(vars(sys)) >= 80)

print(
    "sys_constants",
    sys.api_version == 1013,
    sys.dont_write_bytecode is False,
    sys.float_repr_style == "short",
    sys.pycache_prefix is None,
    isinstance(sys.platlibdir, str) and len(sys.platlibdir) > 0,
)

print(
    "sys_containers",
    isinstance(sys.orig_argv, list) and all(isinstance(x, str) for x in sys.orig_argv),
    isinstance(sys.warnoptions, list),
    isinstance(sys.meta_path, list),
    isinstance(sys.path_hooks, list),
    isinstance(sys.path_importer_cache, dict),
    isinstance(sys._xoptions, dict),
)

print(
    "stdlib_module_names",
    isinstance(sys.stdlib_module_names, frozenset),
    all(name in sys.stdlib_module_names for name in ("math", "sys", "json", "os", "types")),
)

print(
    "thread_info",
    isinstance(sys.thread_info, tuple),
    len(sys.thread_info) == 3,
    sys.thread_info[0] == sys.thread_info.name,
    sys.thread_info.name in ("nt", "pthread", "pthread-stubs", "solaris"),
    sys.thread_info[1] == sys.thread_info.lock,
    sys.thread_info[2] == sys.thread_info.version,
)

print(
    "dunder_streams_and_hooks",
    sys.__stdout__ is sys.stdout,
    sys.__stderr__ is sys.stderr,
    sys.__stdin__ is sys.stdin,
    sys.__displayhook__ is sys.displayhook,
    sys.__excepthook__ is sys.excepthook,
    sys.__unraisablehook__ is sys.unraisablehook,
    sys.__breakpointhook__ is sys.breakpointhook,
)

print(
    "fs_encoding",
    sys.getfilesystemencoding() == "utf-8",
    sys.getfilesystemencodeerrors() in ("surrogatepass", "surrogateescape"),
)

# switchinterval round-trip + validation
orig_interval = sys.getswitchinterval()
print("switchinterval_pos", isinstance(orig_interval, float) and orig_interval > 0)
sys.setswitchinterval(0.01)
print("switchinterval_set", sys.getswitchinterval() == 0.01)
sys.setswitchinterval(orig_interval)
for bad in (0, -1, "x"):
    try:
        sys.setswitchinterval(bad)
    except Exception as exc:
        print(f"switchinterval_err_{bad}", type(exc).__name__, str(exc))

# coroutine_origin_tracking_depth round-trip + validation
print("coro_depth_default", sys.get_coroutine_origin_tracking_depth())
sys.set_coroutine_origin_tracking_depth(4)
print("coro_depth_set", sys.get_coroutine_origin_tracking_depth())
sys.set_coroutine_origin_tracking_depth(0)
for bad in (-1, "x"):
    try:
        sys.set_coroutine_origin_tracking_depth(bad)
    except Exception as exc:
        print(f"coro_depth_err_{bad}", type(exc).__name__, str(exc))

# profile & trace hooks
print("profile_trace_default", sys.getprofile(), sys.gettrace())


def _sample_hook(frame, event, arg):
    return None


sys.setprofile(_sample_hook)
print("profile_set", sys.getprofile() is _sample_hook)
sys.setprofile(None)
print("profile_reset", sys.getprofile())

sys.settrace(_sample_hook)
print("trace_set", sys.gettrace() is _sample_hook)
sys.settrace(None)
print("trace_reset", sys.gettrace())

# asyncgen hooks (tuple-unpackable + named fields + validation)
orig_hooks = sys.get_asyncgen_hooks()
fi0, fin0 = orig_hooks
print("asyncgen_default", isinstance(orig_hooks, tuple), len(orig_hooks), fi0, fin0, orig_hooks.firstiter, orig_hooks.finalizer)


def _fi(agen):
    pass


def _fin(agen):
    pass


sys.set_asyncgen_hooks(firstiter=_fi, finalizer=_fin)
h1 = sys.get_asyncgen_hooks()
print("asyncgen_set", h1.firstiter is _fi, h1.finalizer is _fin, h1[0] is _fi, h1[1] is _fin)
sys.set_asyncgen_hooks(*orig_hooks)
print("asyncgen_restored", sys.get_asyncgen_hooks() == (None, None))

# audit hooks
seen_events = []
sys.addaudithook(
    lambda ev, args: seen_events.append((ev, args)) if ev.startswith("test2970.") else None
)
sys.audit("test2970.ping", 1, "two")
print("audit_events", seen_events)

# call_tracing, getallocatedblocks, _clear_type_cache
print("call_tracing", sys.call_tracing(lambda a, b: a * b, (6, 7)))
print("allocated_blocks", isinstance(sys.getallocatedblocks(), int) and sys.getallocatedblocks() > 0)
print("clear_type_cache", sys._clear_type_cache())

# ── 3. types.ModuleType construction & subclassing ───────────────────────────

print("moduletype_identity", types.ModuleType is type(sys), types.ModuleType is type(math))
print(
    "moduletype_meta",
    repr(types.ModuleType),
    types.ModuleType.__name__,
    types.ModuleType.__module__,
    types.ModuleType.__bases__,
)
print("moduletype_isinstance", isinstance(sys, types.ModuleType), isinstance(math, types.ModuleType))

m = types.ModuleType("demo")
print("m_type", type(m) is types.ModuleType, isinstance(m, types.ModuleType), repr(m))
print("m_slots", list(m.__dict__) == SLOTS)
print("m_values", m.__name__, m.__doc__, m.__package__, m.__loader__, m.__spec__)
print("m_dict_live", m.__dict__ is vars(m), m.__dict__ is m.__dict__)

m.x = 10
m.__dict__["y"] = 20
print("m_live_rw", m.__dict__["x"], m.y)

m_doc = types.ModuleType("documented", "module doc")
print("m_doc", m_doc.__name__, m_doc.__doc__, m_doc.__package__)

m_kw = types.ModuleType(name="kwmod", doc=98765)
print("m_kw", m_kw.__name__, m_kw.__doc__, m_kw.__package__)

# Re-calling __init__ resets the 5 dunder slots while preserving extra attrs.
m.__init__("renamed", "fresh doc")
print("m_reinit", m.__name__, m.__doc__, m.__package__, m.x, m.y, repr(m), list(m.__dict__)[:5] == SLOTS)

# Deleting / replacing __name__ in __dict__ updates repr() like CPython.
m_repr = types.ModuleType("temp_repr")
m_repr.__dict__["__name__"] = 123
print("m_repr_int", repr(m_repr), m_repr.__repr__())
del m_repr.__name__
print("m_repr_del", repr(m_repr), m_repr.__repr__())
m_repr.__dict__.clear()
print("m_cleared", list(m_repr.__dict__), repr(m_repr))

# Exact CPython 3.12 TypeError wording on invalid calls.
for label, fn in (
    ("missing_name", lambda: types.ModuleType()),
    ("bad_name_type", lambda: types.ModuleType(123)),
    ("too_many_pos", lambda: types.ModuleType("a", "b", "c")),
    ("too_many_kw", lambda: types.ModuleType("a", "b", doc="c")),
    ("invalid_kw_no_name", lambda: types.ModuleType(foo=1)),
    ("invalid_kw", lambda: types.ModuleType("a", foo=1)),
    ("dup_name", lambda: types.ModuleType("a", name="b")),
):
    try:
        fn()
    except TypeError as exc:
        print(f"err_{label}", str(exc))


# Subclassing types.ModuleType (both with and without custom __init__).
class PlainSubModule(types.ModuleType):
    pass


ps = PlainSubModule("plain_sub", "plain doc")
print(
    "plain_sub",
    isinstance(ps, PlainSubModule),
    isinstance(ps, types.ModuleType),
    issubclass(PlainSubModule, types.ModuleType),
    repr(ps),
    ps.__name__,
    ps.__doc__,
    ps.__package__,
    list(ps.__dict__) == SLOTS,
    ps.__dict__ is vars(ps),
)


class CustomSubModule(types.ModuleType):
    def __init__(self, name, tag):
        super().__init__(name, "custom doc")
        self.tag = tag

    def describe(self):
        return f"{self.__name__}:{self.tag}"


cs = CustomSubModule("custom_sub", 7)
cs.__dict__["extra"] = "live"
print(
    "custom_sub",
    isinstance(cs, CustomSubModule),
    isinstance(cs, types.ModuleType),
    repr(cs),
    cs.__name__,
    cs.__doc__,
    cs.__package__,
    cs.tag,
    cs.extra,
    cs.describe(),
    list(cs.__dict__)[:5] == SLOTS,
    cs.__dict__ is vars(cs),
)
