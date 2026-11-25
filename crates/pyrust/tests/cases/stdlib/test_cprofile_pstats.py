# Parity test for `_lsprof`, `cProfile`, `profile`, and `pstats` (CPython 3.12, issue #3247).
import _lsprof
import cProfile
import enum
import io
import os
import profile
import pstats
import tempfile


# ── 1. Module exports & SortKey StrEnum ──────────────────────────────────────
print("cProfile.__all__:", sorted(cProfile.__all__))
print("profile.__all__:", sorted(profile.__all__))
print("pstats.__all__:", sorted(pstats.__all__))
print("Profile subclass of _lsprof.Profiler:", issubclass(cProfile.Profile, _lsprof.Profiler))
print("SortKey is StrEnum:", issubclass(pstats.SortKey, enum.StrEnum))
print("SortKey members:", [(m.name, m.value) for m in pstats.SortKey])
print("SortKey aliases:",
      pstats.SortKey("ncalls") is pstats.SortKey.CALLS,
      pstats.SortKey("cumtime") is pstats.SortKey.CUMULATIVE,
      pstats.SortKey("module") is pstats.SortKey.FILENAME,
      pstats.SortKey("tottime") is pstats.SortKey.TIME)


# ── 2. _lsprof.Profiler & cProfile.Profile with deterministic custom timer ───
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)


def leaf(x):
    return x * x + 1


def driver():
    s = 0
    for i in range(4):
        s += leaf(i)
    return s + fib(5)


tick = 0


def fake_timer():
    global tick
    tick += 10
    return tick


pr = cProfile.Profile(timer=fake_timer, timeunit=0.001, builtins=False)
res = pr.runcall(driver)
print("driver result:", res)

entries = pr.getstats()
by_name = {
    (e.code.co_name if hasattr(e.code, "co_name") else e.code): e
    for e in entries
}
print("lsprof driver counts:", by_name["driver"].callcount, by_name["driver"].reccallcount)
print("lsprof leaf counts:", by_name["leaf"].callcount, by_name["leaf"].reccallcount)
print("lsprof fib counts:", by_name["fib"].callcount, by_name["fib"].reccallcount)
print("lsprof entry tuple len:", len(by_name["driver"]), by_name["driver"].n_fields)

# Context manager + runctx
with cProfile.Profile(builtins=False) as ctx_pr:
    ans = leaf(9)
ctx_pr.create_stats()
print("ctx manager ans:", ans)
print("ctx manager saw leaf:", any(k[2] == "leaf" for k in ctx_pr.stats))

ctx_ns = {"leaf": leaf}
pr2 = cProfile.Profile(builtins=False)
pr2.runctx("out = leaf(3) + leaf(4)", ctx_ns, ctx_ns)
pr2.create_stats()
print("runctx out:", ctx_ns["out"])
leaf_entries = [v for k, v in pr2.stats.items() if k[2] == "leaf"]
print("runctx leaf calls:", leaf_entries[0][0], leaf_entries[0][1])


# ── 3. pstats.Stats: strip_dirs, sort_stats, reverse_order, print_*, profile ─
stream = io.StringIO()
st = pstats.Stats(pr, stream=stream)
st.strip_dirs()
st.sort_stats(pstats.SortKey.CALLS, pstats.SortKey.NAME)
sp = st.get_stats_profile()
print("StatsProfile type:", isinstance(sp, pstats.StatsProfile))
print("FunctionProfile type:", isinstance(sp.func_profiles["fib"], pstats.FunctionProfile))
print("fib ncalls:", sp.func_profiles["fib"].ncalls)
print("leaf ncalls:", sp.func_profiles["leaf"].ncalls)
print("driver ncalls:", sp.func_profiles["driver"].ncalls)
print("leaf file stripped:", sp.func_profiles["leaf"].file_name == os.path.basename(__file__))
print("leaf lineno:", sp.func_profiles["leaf"].line_number == leaf.__code__.co_firstlineno)

st.print_stats()
out_stats = stream.getvalue()
print("print_stats header:", "ncalls  tottime  percall  cumtime  percall filename:lineno(function)" in out_stats)
print("print_stats ordered:", "Ordered by: call count, function name" in out_stats)
print("print_stats has fib 15/1:", "15/1" in out_stats and "(fib)" in out_stats)
print("print_stats has leaf:", "(leaf)" in out_stats)

# print_callers & print_callees
c_stream = io.StringIO()
st.stream = c_stream
st.sort_stats("name").print_callers("leaf").print_callees("driver")
c_out = c_stream.getvalue()
print("callers heading:", "was called by..." in c_out)
print("callees heading:", "called..." in c_out)
print("driver calls leaf:", "(leaf)" in c_out and "(driver)" in c_out)

# reverse_order & mixed sort_stats TypeError
names_fwd = [k[2] for k in st.sort_stats("name").fcn_list]
names_rev = [k[2] for k in st.reverse_order().fcn_list]
print("reverse_order works:", names_rev == list(reversed(names_fwd)))
try:
    st.sort_stats("name", pstats.SortKey.CALLS)
    print("ERROR: expected TypeError")
except TypeError as exc:
    print("mixed sort_stats err:", str(exc))

# dump_stats + load from file + Stats.add
with tempfile.TemporaryDirectory() as tmpdir:
    stats_path = os.path.join(tmpdir, "prof.stats")
    st.dump_stats(stats_path)
    loaded = pstats.Stats(stats_path, stream=io.StringIO())
    print("loaded total_calls:", loaded.total_calls == st.total_calls)
    print("loaded prim_calls:", loaded.prim_calls == st.prim_calls)
    loaded.add(stats_path)
    print("added doubles calls:", loaded.total_calls == st.total_calls * 2)

    # Also test cProfile.Profile.dump_stats directly
    pr3 = cProfile.Profile(builtins=False)
    pr3.runcall(leaf, 5)
    stats_path2 = os.path.join(tmpdir, "prof2.stats")
    pr3.dump_stats(stats_path2)
    loaded2 = pstats.Stats(stats_path2, stream=io.StringIO())
    print("Profile.dump_stats roundtrip:", any(k[2] == "leaf" for k in loaded2.stats))


# ── 4. profile.Profile pure-Python-compatible API ────────────────────────────
py_prof = profile.Profile()
py_res = py_prof.runcall(driver)
print("profile.Profile runcall result:", py_res)
py_st = pstats.Stats(py_prof, stream=io.StringIO()).strip_dirs()
py_sp = py_st.get_stats_profile()
print("profile fib ncalls:", py_sp.func_profiles["fib"].ncalls)
print("profile leaf ncalls:", py_sp.func_profiles["leaf"].ncalls)
print("profile driver ncalls:", py_sp.func_profiles["driver"].ncalls)

# Helper functions in pstats
print("func_std_string builtin:", pstats.func_std_string(("~", 0, "<built-in function len>")))
print("func_std_string py:", pstats.func_std_string(("foo.py", 12, "bar")))
print("f8:", repr(pstats.f8(1.23456)))
