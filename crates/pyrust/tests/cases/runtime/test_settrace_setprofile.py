# Parity test for live `sys.settrace()` and `sys.setprofile()` VM hooks (CPython 3.12, issue #3190).
import sys


# ── 1. Basic settrace: call, line, return, exception, local trace return ────
trace_events = []


def local_trace(frame, event, arg):
    if event == "exception":
        detail = arg[0].__name__
    else:
        detail = repr(arg)
    trace_events.append((frame.f_code.co_name, event, frame.f_lineno, detail))
    return local_trace


def global_trace(frame, event, arg):
    trace_events.append(("GLOBAL", frame.f_code.co_name, event, frame.f_lineno, arg))
    if frame.f_code.co_name == "skip_me":
        return None
    return local_trace


def skip_me():
    x = 10
    return x + 5


def catch_error():
    try: return 1 // 0
    except ZeroDivisionError:
        return 42


def target(n):
    a = skip_me()
    b = catch_error()
    return n + a + b


print("initial gettrace:", sys.gettrace())
sys.settrace(global_trace)
print("active gettrace:", sys.gettrace() is global_trace)
result = target(1)
sys.settrace(None)
print("cleared gettrace:", sys.gettrace())
print("target result:", result)
for ev in trace_events:
    print("trace:", ev)


# ── 2. Uncaught exception propagating across frames ─────────────────────────
unwind_events = []


def unwind_trace(frame, event, arg):
    detail = arg[0].__name__ if event == "exception" else repr(arg)
    unwind_events.append((frame.f_code.co_name, event, detail))
    return unwind_trace


def inner_raise():
    raise ValueError("boom")


def outer_catch():
    try: inner_raise()
    except ValueError:
        return "caught"


sys.settrace(unwind_trace)
print("outer_catch:", outer_catch())
sys.settrace(None)
for ev in unwind_events:
    print("unwind:", ev)


# ── 3. Local trace switching & returning None keeps existing local trace ────
switch_events = []


def trace_phase_b(frame, event, arg):
    switch_events.append(("B", frame.f_code.co_name, event))
    return None  # returning None preserves trace_phase_b


def trace_phase_a(frame, event, arg):
    switch_events.append(("A", frame.f_code.co_name, event))
    if event == "line":
        return trace_phase_b
    return trace_phase_a


def global_switch(frame, event, arg):
    return trace_phase_a


def three_lines():
    x = 1
    y = 2
    return x + y


sys.settrace(global_switch)
print("three_lines:", three_lines())
sys.settrace(None)
print("switch_events:", switch_events)


# ── 4. frame.f_trace read/write & frame.f_trace_lines = False ───────────────
ftrace_events = []


def record_only(frame, event, arg):
    ftrace_events.append((frame.f_code.co_name, event))
    return record_only


def clear_own_trace():
    x = 1
    f = sys._getframe()
    seen = f.f_trace is record_only
    f.f_trace = None
    y = 2
    return (seen, x + y)


sys.settrace(record_only)
print("clear_own_trace:", clear_own_trace())
sys.settrace(None)
print("ftrace_events:", ftrace_events)

no_lines_events = []


def disable_lines_trace(frame, event, arg):
    no_lines_events.append((frame.f_code.co_name, event))
    frame.f_trace_lines = False
    return disable_lines_trace


def multi_line_fn():
    a = 10
    b = 20
    return a + b


sys.settrace(disable_lines_trace)
print("multi_line_fn:", multi_line_fn())
sys.settrace(None)
print("no_lines_events:", no_lines_events)

# f_trace_lines type validation
cur = sys._getframe()
print("default f_trace_lines:", cur.f_trace_lines, cur.f_trace_opcodes)
try:
    cur.f_trace_lines = 1
    print("ERROR: expected TypeError")
except TypeError as exc:
    print("f_trace_lines type err:", str(exc))


# ── 5. Exception inside trace function clears sys.gettrace() ────────────────
def failing_trace(frame, event, arg):
    raise RuntimeError("trace failure")


def victim():
    return 99


sys.settrace(failing_trace)
try:
    victim()
    print("ERROR: expected RuntimeError")
except RuntimeError as exc:
    print("trace error caught:", str(exc), "gettrace:", sys.gettrace())


# ── 6. Re-entrancy guard: helper calls inside trace function are not traced ─
reentrant_log = []


def trace_helper(n):
    return n * 2


def reentrant_trace(frame, event, arg):
    val = trace_helper(3)
    reentrant_log.append((frame.f_code.co_name, event, val))
    return reentrant_trace


def simple_target():
    return 7


sys.settrace(reentrant_trace)
print("simple_target:", simple_target())
sys.settrace(None)
print("reentrant_log:", reentrant_log)


# ── 7. Loop line tracing ────────────────────────────────────────────────────
loop_lines = []


def loop_trace(frame, event, arg):
    if event == "line":
        loop_lines.append(frame.f_lineno)
    return loop_trace


def sum_three():
    total = 0
    for i in range(3):
        total += i
    return total


sys.settrace(loop_trace)
print("sum_three:", sum_three())
sys.settrace(None)
# Verify loop body line appears 3 times
print("loop_lines count:", len(loop_lines))


# ── 8. sys.setprofile: call, return, c_call, c_return, c_exception ──────────
prof_events = []


def profiler(frame, event, arg):
    if event.startswith("c_"):
        name = getattr(arg, "__name__", repr(arg))
        prof_events.append((event, name))
    else:
        prof_events.append((event, frame.f_code.co_name))


def prof_child(xs):
    return len(xs)


def prof_parent():
    n = prof_child([1, 2, 3])
    try:
        abs("not_a_number")
    except TypeError:
        pass
    return n


print("initial getprofile:", sys.getprofile())
sys.setprofile(profiler)
print("active getprofile:", sys.getprofile() is profiler)
print("prof_parent:", prof_parent())
sys.setprofile(None)
print("cleared getprofile:", sys.getprofile())
print("prof_events:", prof_events)


# ── 9. Exception inside profile function clears sys.getprofile() ────────────
def failing_profile(frame, event, arg):
    raise RuntimeError("profile failure")


sys.setprofile(failing_profile)
try:
    victim()
    print("ERROR: expected RuntimeError")
except RuntimeError as exc:
    print("profile error caught:", str(exc), "getprofile:", sys.getprofile())
