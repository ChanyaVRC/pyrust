# Parity test for `trace` module (CPython 3.12, issue #3247).
import io
import os
import sys
import tempfile
import trace


print("trace.__all__:", sorted(trace.__all__))


def leaf_compute(x):
    if x % 2 == 0:
        return x * 2
    else:
        return x + 1


def loop_compute(n):
    total = 0
    for i in range(n):
        total += leaf_compute(i)
    return total


class Accumulator:
    def step(self, v):
        return leaf_compute(v)


# ── 1. Line counting via Trace(count=1, trace=0).runfunc ─────────────────────
tracer = trace.Trace(count=1, trace=0)
res = tracer.runfunc(loop_compute, 4)
print("loop_compute result:", res)
results = tracer.results()
print("isinstance CoverageResults:", isinstance(results, trace.CoverageResults))

base_line = loop_compute.__code__.co_firstlineno
loop_body_line = base_line + 3  # `total += leaf_compute(i)`
return_line = base_line + 4     # `return total`
print("loop body count:", results.counts.get((__file__, loop_body_line)))
print("return line count:", results.counts.get((__file__, return_line)))

leaf_base = leaf_compute.__code__.co_firstlineno
even_line = leaf_base + 2  # `return x * 2`
odd_line = leaf_base + 4   # `return x + 1`
print("even branch count:", results.counts.get((__file__, even_line)))
print("odd branch count:", results.counts.get((__file__, odd_line)))


# ── 2. Trace(count=0, trace=1) stdout tracing ────────────────────────────────
buf = io.StringIO()
orig_stdout = sys.stdout
try:
    sys.stdout = buf
    t_trace = trace.Trace(count=0, trace=1)
    t_trace.runfunc(leaf_compute, 2)
finally:
    sys.stdout = orig_stdout

trace_out = buf.getvalue()
bname = os.path.basename(__file__)
modname = os.path.splitext(bname)[0]
print("trace saw modulename header:", f" --- modulename: {modname}, funcname: leaf_compute" in trace_out)
print("trace saw even line:", f"{bname}({even_line}):" in trace_out)


# ── 3. countfuncs=1 ──────────────────────────────────────────────────────────
tf = trace.Trace(count=0, trace=0, countfuncs=1)
acc = Accumulator()
print("acc.step(3):", tf.runfunc(acc.step, 3))
rf = tf.results()
called_names = sorted(fn for (_, _, fn) in rf.calledfuncs)
print("calledfuncs names:", called_names)


# ── 4. countcallers=1 ────────────────────────────────────────────────────────
tc = trace.Trace(count=0, trace=0, countcallers=1)
tc.runfunc(loop_compute, 2)
rc = tc.results()
caller_pairs = sorted((p[2], c[2]) for (p, c) in rc.callers if p[2] in ("loop_compute", "leaf_compute") and c[2] in ("loop_compute", "leaf_compute"))
print("caller pairs:", caller_pairs)


# ── 5. CoverageResults.update & write_results (coverdir + outfile/infile) ────
r1 = trace.CoverageResults(counts={("a.py", 1): 2}, calledfuncs={("a.py", "a", "f"): 1})
r2 = trace.CoverageResults(counts={("a.py", 1): 3, ("a.py", 2): 1}, callers={(("a.py", "a", "f"), ("a.py", "a", "g")): 1})
r1.update(r2)
print("merged counts:", sorted(r1.counts.items()))
print("merged calledfuncs:", sorted(r1.calledfuncs.keys()))
print("merged callers:", sorted(r1.callers.keys()))

with tempfile.TemporaryDirectory() as tmpdir:
    counts_file = os.path.join(tmpdir, "counts.pkl")
    cover_dir = os.path.join(tmpdir, "cover")
    t_save = trace.Trace(count=1, trace=0, outfile=counts_file)
    t_save.runfunc(loop_compute, 3)
    res_save = t_save.results()

    sum_buf = io.StringIO()
    orig_stdout = sys.stdout
    try:
        sys.stdout = sum_buf
        res_save.write_results(show_missing=True, summary=True, coverdir=cover_dir)
    finally:
        sys.stdout = orig_stdout

    print("counts file written:", os.path.isfile(counts_file))
    cover_files = os.listdir(cover_dir)
    print("cover file created:", len(cover_files) == 1 and cover_files[0].endswith(".cover"))
    print("summary header:", "lines   cov%   module   (path)" in sum_buf.getvalue())

    reloaded = trace.CoverageResults(infile=counts_file)
    print("reloaded loop body count:", reloaded.counts.get((__file__, loop_body_line)))

# ── 6. runctx ────────────────────────────────────────────────────────────────
ns = {"loop_compute": loop_compute, "__file__": __file__}
t_ctx = trace.Trace(count=1, trace=0)
t_ctx.runctx("ans = loop_compute(3)", ns, ns)
print("runctx ans:", ns["ans"])
print("runctx counted loop body:", t_ctx.results().counts.get((__file__, loop_body_line)) == 3)
