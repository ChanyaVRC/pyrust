# Parity test for remaining `sys` runtime APIs (CPython 3.12, issue #3246):
# - sys.getrefcount
# - sys._getframemodulename
# - sys._current_frames
# - sys._current_exceptions
# - sys.monitoring (PEP 669)
# - Windows-only sys.getwindowsversion / sys.winver / sys.dllhandle
import sys
import threading


# 1. sys.getrefcount
class Box:
    pass


b = Box()
rc1 = sys.getrefcount(b)
b2 = b
rc2 = sys.getrefcount(b)
del b2
rc3 = sys.getrefcount(b)
print("getrefcount_box:", isinstance(rc1, int), rc1 >= 2, rc2 == rc1 + 1, rc3 == rc1)

lst = [1, 2, 3]
lrc1 = sys.getrefcount(lst)
lst_alias = lst
lrc2 = sys.getrefcount(lst)
del lst_alias
lrc3 = sys.getrefcount(lst)
print("getrefcount_list:", lrc1 >= 2, lrc2 == lrc1 + 1, lrc3 == lrc1)

print("getrefcount_none:", isinstance(sys.getrefcount(None), int), sys.getrefcount(None) >= 2)

# 2. sys._getframemodulename
print("framemod_top:", sys._getframemodulename(), sys._getframemodulename(0))


def nested_modname():
    return sys._getframemodulename(0), sys._getframemodulename(1)


print("framemod_nested:", nested_modname())
print("framemod_deep:", sys._getframemodulename(9999))

# 3. sys._current_frames
frames = sys._current_frames()
tid = threading.get_ident()
print("current_frames_dict:", isinstance(frames, dict), tid in frames)
cur_f = frames[tid]
print("current_frames_code:", cur_f.f_code.co_name == "<module>")

# 4. sys._current_exceptions
excs_none = sys._current_exceptions()
print("current_exc_outside:", isinstance(excs_none, dict), tid in excs_none, excs_none[tid] is None)

try:
    raise ValueError("boom")
except ValueError as err:
    excs_in = sys._current_exceptions()
    print("current_exc_inside:", excs_in[tid] is err)

# 5. sys.monitoring (PEP 669)
mon = sys.monitoring
print("mon_ids:", mon.DEBUGGER_ID, mon.COVERAGE_ID, mon.PROFILER_ID, mon.OPTIMIZER_ID)
print("mon_sentinels:", type(mon.MISSING) is object, type(mon.DISABLE) is object, mon.MISSING is not mon.DISABLE)

E = mon.events
required_events = [
    "NO_EVENTS",
    "PY_START",
    "PY_RESUME",
    "PY_RETURN",
    "PY_YIELD",
    "CALL",
    "LINE",
    "INSTRUCTION",
    "JUMP",
    "BRANCH",
    "STOP_ITERATION",
    "RAISE",
    "EXCEPTION_HANDLED",
    "PY_UNWIND",
    "PY_THROW",
    "RERAISE",
    "C_RETURN",
    "C_RAISE",
]
print("mon_events_present:", all(hasattr(E, name) for name in required_events))
print("mon_no_events:", E.NO_EVENTS == 0)
nonzero_bits = [getattr(E, name) for name in required_events if name != "NO_EVENTS"]
print("mon_events_powers_of_2:", all(b > 0 and (b & (b - 1)) == 0 for b in nonzero_bits))
print("mon_events_distinct:", len(set(nonzero_bits)) == len(nonzero_bits))

# Tool registration lifecycle
tid_tool = 3
print("tool_initial:", mon.get_tool(tid_tool))
mon.use_tool_id(tid_tool, "test_tool")
print("tool_used:", mon.get_tool(tid_tool))

try:
    mon.use_tool_id(tid_tool, "dup_tool")
    print("tool_dup: none")
except ValueError:
    print("tool_dup: ValueError")

try:
    mon.use_tool_id(6, "bad_id")
    print("tool_bad_id: none")
except ValueError:
    print("tool_bad_id: ValueError")

# Global events
print("events_initial:", mon.get_events(tid_tool))
mon.set_events(tid_tool, E.PY_START | E.LINE)
print("events_set:", mon.get_events(tid_tool) == (E.PY_START | E.LINE))

try:
    mon.set_events(tid_tool, E.C_RETURN)
    print("c_return_set: none")
except ValueError:
    print("c_return_set: ValueError")

# Local events
def target_fn():
    return 123


print("local_initial:", mon.get_local_events(tid_tool, target_fn.__code__))
mon.set_local_events(tid_tool, target_fn.__code__, E.PY_START | E.PY_RETURN)
print("local_set:", mon.get_local_events(tid_tool, target_fn.__code__) == (E.PY_START | E.PY_RETURN))

try:
    mon.set_local_events(tid_tool, target_fn.__code__, E.RAISE)
    print("local_raise: none")
except ValueError:
    print("local_raise: ValueError")

# Callback registration
def cb1(*args):
    return None


def cb2(*args):
    return None


prev0 = mon.register_callback(tid_tool, E.PY_START, cb1)
prev1 = mon.register_callback(tid_tool, E.PY_START, cb2)
prev2 = mon.register_callback(tid_tool, E.PY_START, None)
print("callbacks_prev:", prev0 is None, prev1 is cb1, prev2 is cb2)

try:
    mon.register_callback(tid_tool, E.PY_START | E.LINE, cb1)
    print("cb_multi: none")
except ValueError:
    print("cb_multi: ValueError")

mon.set_local_events(tid_tool, target_fn.__code__, 0)
mon.restart_events()
mon.free_tool_id(tid_tool)
print("tool_freed:", mon.get_tool(tid_tool), mon.get_events(tid_tool), mon.get_local_events(tid_tool, target_fn.__code__))

# 6. Windows-specific sys attributes
if sys.platform == "win32":
    wv = sys.getwindowsversion()
    print("wv_tuple:", isinstance(wv, tuple), len(wv), wv.n_sequence_fields, wv.n_fields)
    print("wv_fields_types:", isinstance(wv.major, int), isinstance(wv.minor, int), isinstance(wv.build, int), isinstance(wv.platform, int), isinstance(wv.service_pack, str))
    print("wv_extra_types:", isinstance(wv.service_pack_major, int), isinstance(wv.service_pack_minor, int), isinstance(wv.suite_mask, int), isinstance(wv.product_type, int), isinstance(wv.platform_version, tuple))
    print("winver_match:", sys.winver == f"{sys.version_info.major}.{sys.version_info.minor}")
    print("dllhandle_int:", isinstance(sys.dllhandle, int), sys.dllhandle != 0)
else:
    print("unix_no_win_attrs:", hasattr(sys, "getwindowsversion"), hasattr(sys, "winver"), hasattr(sys, "dllhandle"))
