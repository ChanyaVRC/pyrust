# Comprehensive CPython 3.12 parity tests for `timeit` (issue #3227).

import io
import sys
import time
import timeit
import warnings


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, str(e))


def make_fake_timer(step=0.01):
    state = [0.0]

    def clock():
        val = state[0]
        state[0] += step
        return val

    return clock


# 1. Module metadata and constants
print("in sys.modules:", "timeit" in sys.modules)
print("__all__:", timeit.__all__)
print("defaults:", timeit.default_number, timeit.default_repeat, timeit.dummy_src_name)
print("default_timer is perf_counter:", timeit.default_timer is time.perf_counter)
print("Timer.__module__:", timeit.Timer.__module__)
print("reindent:", repr(timeit.reindent("a\nb\nc", 4)))

# 2. Timer with string stmt and setup + fake timer
t = timeit.Timer("x += 1", "x = 0", timer=make_fake_timer(0.25))
print("str timeit:", round(t.timeit(10), 6))
print("str repeat:", [round(v, 6) for v in t.repeat(3, 10)])

# Multi-line stmt and setup
ml_t = timeit.Timer(
    "y = x + 1\nz = y * 2",
    "a = 1\nx = a + 2",
    timer=make_fake_timer(0.5),
)
print("multiline timeit:", round(ml_t.timeit(5), 6))

# 3. Timer with callable stmt and setup (and mixed)
calls = []


def my_setup():
    calls.append("setup")


def my_stmt():
    calls.append("stmt")


ct = timeit.Timer(my_stmt, my_setup, timer=make_fake_timer(0.1))
elapsed = ct.timeit(4)
print("callable timeit:", round(elapsed, 6), calls)

calls.clear()
ct_rep = timeit.Timer(my_stmt, my_setup, timer=make_fake_timer(0.1))
rep = ct_rep.repeat(2, 3)
print("callable repeat:", [round(v, 6) for v in rep], calls)

# Mixed: string setup + callable stmt, callable setup + string stmt
mix_log = []
t_mix1 = timeit.Timer(lambda: mix_log.append(1), "a = 1", timer=make_fake_timer(0.2))
print("mix1:", round(t_mix1.timeit(3), 6), len(mix_log))

mix_log.clear()
t_mix2 = timeit.Timer("1 + 1", lambda: mix_log.append("s"), timer=make_fake_timer(0.2))
print("mix2:", round(t_mix2.timeit(3), 6), mix_log)

# 4. Timer with globals namespace
ns = {"factor": 7, "acc": []}
gt = timeit.Timer("acc.append(factor)", globals=ns, timer=make_fake_timer(0.05))
print("globals timeit:", round(gt.timeit(4), 6), ns["acc"])

# 5. Validation errors
print("bad stmt:", expect_exc(timeit.Timer, 123))
print("bad setup:", expect_exc(timeit.Timer, "pass", 456))
print("syntax err stmt:", expect_exc(timeit.Timer, "def (")[0])
print("syntax err setup:", expect_exc(timeit.Timer, "pass", "def (")[0])
print("return in stmt:", expect_exc(timeit.Timer, "return 1")[0])
print("return in setup:", expect_exc(timeit.Timer, "pass", "return 1")[0])

# 6. autorange with deterministic fake timer (scales with number of iterations)
class IterClock:
    def __init__(self, per_iter):
        self.t = 0.0
        self.per_iter = per_iter

    def tick(self):
        self.t += self.per_iter

    def clock(self):
        return self.t


ic = IterClock(0.005)
auto_t = timeit.Timer(ic.tick, timer=ic.clock)
cb_log = []
auto_res = auto_t.autorange(lambda n, dt: cb_log.append((n, round(dt, 6))))
print("autorange res:", (auto_res[0], round(auto_res[1], 6)))
print("autorange cb:", cb_log)

# autorange without callback
ic2 = IterClock(0.025)
auto_t2 = timeit.Timer(ic2.tick, timer=ic2.clock)
auto_res2 = auto_t2.autorange()
print("autorange no cb:", (auto_res2[0], round(auto_res2[1], 6)))

# 7. print_exc displays traceback with <timeit-src> and exception type
err_t = timeit.Timer("1 / 0")
exc_buf = io.StringIO()
try:
    err_t.timeit(1)
except ZeroDivisionError:
    err_t.print_exc(file=exc_buf)
exc_text = exc_buf.getvalue()
print("print_exc has dummy_src_name:", "<timeit-src>" in exc_text)
print("print_exc has ZeroDivisionError:", "ZeroDivisionError" in exc_text)
print("print_exc has stmt line:", "1 / 0" in exc_text)

# 8. Module-level timeit() and repeat() convenience functions
real_dt = timeit.timeit("1 + 1", number=100)
print("real timeit float >= 0:", isinstance(real_dt, float) and real_dt >= 0.0)
real_rep = timeit.repeat("1 + 1", repeat=3, number=50)
print(
    "real repeat list:",
    isinstance(real_rep, list) and len(real_rep) == 3 and all(isinstance(x, float) and x >= 0.0 for x in real_rep),
)
print(
    "mod timeit fake:",
    round(timeit.timeit("x + 1", setup="x = 41", timer=make_fake_timer(0.125), number=5), 6),
)
print(
    "mod repeat fake:",
    [round(v, 6) for v in timeit.repeat("x + 1", setup="x = 41", timer=make_fake_timer(0.125), repeat=2, number=5)],
)

# 9. timeit.main() CLI interface
def run_main(argv, wrap_timer=None):
    out = io.StringIO()
    err = io.StringIO()
    old_out, old_err = sys.stdout, sys.stderr
    old_path = list(sys.path)
    try:
        sys.stdout, sys.stderr = out, err
        rc = timeit.main(argv, _wrap_timer=wrap_timer)
    finally:
        sys.stdout, sys.stderr = old_out, old_err
        sys.path[:] = old_path
    return rc, out.getvalue(), err.getvalue()


rc, out, err = run_main(["-n", "10", "-r", "3", "1+1"], wrap_timer=lambda _: make_fake_timer(0.005))
print("main basic:", rc, repr(out), repr(err))

rc, out, err = run_main(
    ["--number=5", "--repeat=2", "--setup=x = 1", "--unit=msec", "x + 1"],
    wrap_timer=lambda _: make_fake_timer(0.01),
)
print("main long opts:", rc, repr(out), repr(err))

rc, out, err = run_main(
    ["-n", "2", "-r", "2", "-v", "-u", "usec", "pass"],
    wrap_timer=lambda _: make_fake_timer(0.002),
)
print("main verbose:", rc, repr(out), repr(err))

rc, out, err = run_main(["-u", "bogus", "pass"])
print("main bad unit:", rc, repr(out), repr(err))

rc, out, err = run_main(["-Z"])
print("main unknown opt:", rc, "not recognized" in out, "use -h/--help" in out)

rc, out, err = run_main(["-h"])
print("main help:", rc, "Command line usage:" in out, repr(err))

rc, out, err = run_main(["-n", "1", "-r", "1", "1/0"])
print("main error:", rc, repr(out), "ZeroDivisionError" in err)

# Unreliable results warning (worst >= best * 4)
skewed_times = iter([0.0, 1.0, 0.0, 5.0])
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    rc, out, err = run_main(["-n", "1", "-r", "2", "pass"], wrap_timer=lambda _: lambda: next(skewed_times))
print("main unreliable warning:", rc, len(caught), "likely unreliable" in str(caught[0].message))
