# Parity test for `tracemalloc` & `_tracemalloc` stdlib modules (CPython 3.12, issue #3246).
import os
import tempfile
import _tracemalloc
import tracemalloc


# 1. Initial stopped state
tracemalloc.stop()
print("initial_tracing:", tracemalloc.is_tracing(), _tracemalloc.is_tracing())
print("stopped_memory:", tracemalloc.get_traced_memory())
print("stopped_tm_mem:", tracemalloc.get_tracemalloc_memory() >= 0)

try:
    tracemalloc.take_snapshot()
    print("snap_when_stopped: unexpected")
except RuntimeError:
    print("snap_when_stopped: RuntimeError")

try:
    tracemalloc.start(0)
    print("start_zero: unexpected")
except ValueError:
    print("start_zero: ValueError")

# 2. Start tracing, allocate objects, inspect memory & traceback
tracemalloc.start(5)
print("started:", tracemalloc.is_tracing(), tracemalloc.get_traceback_limit())

buf1 = [i for i in range(200)]
buf2 = {f"key_{i}": i * 2 for i in range(100)}
buf3 = bytearray(4096)

cur, peak = tracemalloc.get_traced_memory()
print("traced_mem_positive:", cur > 0, peak >= cur)
print("tm_mem_positive:", tracemalloc.get_tracemalloc_memory() > 0)

snap1 = tracemalloc.take_snapshot()
print("snap1_limit:", snap1.traceback_limit)
print("snap1_traces_nonempty:", len(snap1.traces) > 0)

# Extra allocation increases peak; reset_peak lowers peak to current
big_list = [x for x in range(500)]
cur2, peak2 = tracemalloc.get_traced_memory()
print("peak_grew:", peak2 >= peak)
tracemalloc.reset_peak()
cur3, peak3 = tracemalloc.get_traced_memory()
print("after_reset_peak:", peak3 >= cur3)

snap2 = tracemalloc.take_snapshot()

# 3. Object traceback
tb_obj = tracemalloc.get_object_traceback(buf1)
print(
    "obj_tb_valid:",
    tb_obj is None
    or (
        isinstance(tb_obj, tracemalloc.Traceback)
        and len(tb_obj) >= 1
        and isinstance(tb_obj[0], tracemalloc.Frame)
    ),
)

# 4. Frame / Traceback / Trace / Statistic / StatisticDiff value semantics
f1 = tracemalloc.Frame(("alpha.py", 10))
f2 = tracemalloc.Frame(("alpha.py", 20))
f3 = tracemalloc.Frame(("beta.py", 5))
print("frame_attrs:", f1.filename, f1.lineno)
print("frame_str:", str(f1))
print("frame_repr:", repr(f1))
print("frame_cmp:", f1 == tracemalloc.Frame(("alpha.py", 10)), f1 < f2, f2 < f3, f1 != f2)
print("frame_hash:", hash(f1) == hash(tracemalloc.Frame(("alpha.py", 10))))

tb = tracemalloc.Traceback((("alpha.py", 10), ("beta.py", 5)), total_nframe=4)
print("tb_len:", len(tb), "total_nframe:", tb.total_nframe)
print("tb_index:", tb[0] == f3, tb[1] == f1, tb[-1] == f1)
print("tb_slice:", len(tb[:1]), tb[:1][0] == f3)
print("tb_contains:", f1 in tb, f2 in tb)
print("tb_str:", str(tb))
print("tb_repr:", repr(tb))
print("tb_eq_hash:", tb == tracemalloc.Traceback((("alpha.py", 10), ("beta.py", 5))), hash(tb) == hash(tracemalloc.Traceback((("alpha.py", 10), ("beta.py", 5)))))
fmt_default = tb.format()
fmt_mrf = tb.format(most_recent_first=True)
fmt_lim = tb.format(limit=1)
print("tb_format_order:", fmt_default[0], "|", fmt_mrf[0], "|", len(fmt_lim))

tr = tracemalloc.Trace((0, 2048, (("alpha.py", 10), ("beta.py", 5)), 2))
print("trace_attrs:", tr.domain, tr.size, len(tr.traceback), tr.traceback.total_nframe)
print("trace_str:", str(tr))
print("trace_repr:", repr(tr))
print("trace_eq:", tr == tracemalloc.Trace((0, 2048, (("alpha.py", 10), ("beta.py", 5)), 2)))

st1 = tracemalloc.Statistic(tb, 2048, 2)
print("stat_attrs:", st1.size, st1.count, st1.traceback == tb)
print("stat_str:", str(st1))
print("stat_repr:", repr(st1))

sd1 = tracemalloc.StatisticDiff(tb, 4096, 2048, 4, 2)
print("statdiff_attrs:", sd1.size, sd1.size_diff, sd1.count, sd1.count_diff)
print("statdiff_str:", str(sd1))
print("statdiff_repr:", repr(sd1))

# 5. _format_size parity
for val, sign in [(0, False), (512, False), (1024, False), (1536, True), (-1536, True), (20480, False), (5 * 1024 * 1024, False)]:
    print("fmt_size:", val, sign, tracemalloc._format_size(val, sign))

# 6. Synthetic Snapshot: statistics, compare_to, filter_traces, dump/load
raw_old = [
    (0, 1000, (("mod_a.py", 10), ("caller.py", 1)), 2),
    (0, 500, (("mod_a.py", 10), ("caller.py", 1)), 2),
    (1, 2000, (("mod_b.py", 20), ("caller.py", 2)), 2),
]
raw_new = [
    (0, 3000, (("mod_a.py", 10), ("caller.py", 1)), 2),
    (1, 2000, (("mod_b.py", 20), ("caller.py", 2)), 2),
    (0, 400, (("mod_c.py", 30), ("caller.py", 3)), 2),
]
s_old = tracemalloc.Snapshot(raw_old, 5)
s_new = tracemalloc.Snapshot(raw_new, 5)

stats_line = s_old.statistics("lineno")
print("stats_line_len:", len(stats_line))
print("stats_line_top:", stats_line[0].size, stats_line[0].count, str(stats_line[0].traceback))
print("stats_line_second:", stats_line[1].size, stats_line[1].count, str(stats_line[1].traceback))

stats_file_cum = s_old.statistics("filename", cumulative=True)
print("stats_cum_has_caller:", any(s.traceback[0].filename == "caller.py" and s.size == 3500 for s in stats_file_cum))

try:
    s_old.statistics("bogus")
except ValueError:
    print("bad_key_type: ValueError")

try:
    s_old.statistics("traceback", cumulative=True)
except ValueError:
    print("cum_traceback_err: ValueError")

diffs = s_new.compare_to(s_old, "lineno")
print("diffs_len:", len(diffs))
print("diffs_top:", diffs[0].size, diffs[0].size_diff, diffs[0].count, diffs[0].count_diff)

# Filtering with Filter & DomainFilter
f_inc = tracemalloc.Filter(True, "mod_a.py")
f_exc = tracemalloc.Filter(False, "mod_b.py")
df_dom = tracemalloc.DomainFilter(True, 1)

s_only_a = s_new.filter_traces([f_inc])
print("filter_inc_a:", len(s_only_a.traces), s_only_a.traces[0].size)
s_no_b = s_new.filter_traces([f_exc])
print("filter_exc_b:", len(s_no_b.traces))
s_dom1 = s_new.filter_traces([df_dom])
print("filter_dom1:", len(s_dom1.traces), s_dom1.traces[0].domain)

# Snapshot dump & load round-trip
tmp_fd, tmp_path = tempfile.mkstemp(suffix=".snap")
os.close(tmp_fd)
try:
    s_new.dump(tmp_path)
    loaded = tracemalloc.Snapshot.load(tmp_path)
    print("loaded_limit:", loaded.traceback_limit)
    print("loaded_traces_len:", len(loaded.traces))
    print("loaded_stats_eq:", loaded.statistics("lineno") == s_new.statistics("lineno"))
finally:
    if os.path.exists(tmp_path):
        os.unlink(tmp_path)

# 7. clear_traces & stop
tracemalloc.clear_traces()
cur_after_clear, peak_after_clear = tracemalloc.get_traced_memory()
print("cleared_mem_le:", cur_after_clear <= cur2 and peak_after_clear >= cur_after_clear)
tracemalloc.stop()
print("stopped_final:", tracemalloc.is_tracing(), tracemalloc.get_traced_memory())
