# Parity test for `bdb`, `pdb`, and default `sys.breakpointhook` wiring (CPython 3.12, issue #3247).
import bdb
import cmd
import io
import os
import pdb
import sys


def norm(text):
    canon = bdb.Bdb().canonic(__file__)
    text = text.replace(canon, "<TESTFILE>")
    text = text.replace(__file__, "<TESTFILE>")
    text = text.replace(os.path.abspath(__file__), "<TESTFILE>")
    return text


# ── 1. Module exports & class hierarchy ──────────────────────────────────────
print("bdb.__all__:", sorted(bdb.__all__))
print("pdb.__all__:", sorted(pdb.__all__))
print("BdbQuit is Exception:", issubclass(bdb.BdbQuit, Exception))
print("Pdb is Bdb:", issubclass(pdb.Pdb, bdb.Bdb))
print("Pdb is Cmd:", issubclass(pdb.Pdb, cmd.Cmd))
print("canonic <string>:", bdb.Bdb().canonic("<string>"))


# ── 2. Breakpoint lifecycle & formatting ─────────────────────────────────────
bdb.Breakpoint.clearBreakpoints()
bp1 = bdb.Breakpoint("demo.py", 10)
bp2 = bdb.Breakpoint("demo.py", 10, temporary=True, cond="x > 0")
print("bp1 str:", str(bp1))
print("bp1 num/enabled:", bp1.number, bp1.enabled, bp1.temporary)
print("bp2 str:", str(bp2))
print("bp1 format:", bp1.bpformat())
bp1.disable()
print("bp1 disabled format:", bp1.bpformat())
bp1.enable()
bp2.ignore = 2
bp2.hits = 3
print("bp2 format:", bp2.bpformat())
buf = io.StringIO()
bp1.bpprint(buf)
print("bpprint matches:", buf.getvalue().strip() == bp1.bpformat())
bp1.deleteMe()
print("after deleteMe bpbynumber[1]:", bdb.Breakpoint.bpbynumber[1])
print("remaining at demo.py:10:", len(bdb.Breakpoint.bplist[("demo.py", 10)]))
bdb.Breakpoint.clearBreakpoints()
print("cleared next/len:", bdb.Breakpoint.next, len(bdb.Breakpoint.bpbynumber), len(bdb.Breakpoint.bplist))


# ── 3. Custom Bdb subclass: stepping, calls, returns, exceptions, quit ───────
class RecordingBdb(bdb.Bdb):
    def __init__(self, skip=None, quit_on_func=None):
        super().__init__(skip=skip)
        self.events = []
        self.quit_on_func = quit_on_func

    def user_call(self, frame, arg):
        self.events.append(("call", frame.f_code.co_name))
        if self.quit_on_func and frame.f_code.co_name == self.quit_on_func:
            self.set_quit()

    def user_line(self, frame):
        self.events.append(("line", frame.f_code.co_name))

    def user_return(self, frame, return_value):
        self.events.append(("return", frame.f_code.co_name, return_value))

    def user_exception(self, frame, exc_info):
        self.events.append(("exception", frame.f_code.co_name, exc_info[0].__name__))


def helper_add(a, b):
    total = a + b
    return total


def target_work(n):
    x = helper_add(n, 1)
    try:
        y = 1 // 0
    except ZeroDivisionError:
        y = 10
    return x + y


rec = RecordingBdb()
res = rec.runcall(target_work, 5)
print("runcall result:", res)
print("has helper_add call:", ("call", "helper_add") in rec.events)
print("has helper_add return 6:", ("return", "helper_add", 6) in rec.events)
print("has ZeroDivisionError:", ("exception", "target_work", "ZeroDivisionError") in rec.events)
print("has target_work return 16:", ("return", "target_work", 16) in rec.events)

# Bdb.run and Bdb.runeval
ns = {}
rec2 = RecordingBdb()
rec2.run("v = 2 + 3\nw = v * 4", ns, ns)
print("Bdb.run ns:", ns["v"], ns["w"])
print("Bdb.run saw module lines:", ("line", "<module>") in rec2.events)

rec3 = RecordingBdb()
eval_res = rec3.runeval("v + w", ns, ns)
print("Bdb.runeval:", eval_res)

# Bdb.set_quit aborts via BdbQuit cleanly
rec_quit = RecordingBdb(quit_on_func="helper_add")
quit_res = rec_quit.runcall(target_work, 5)
print("Bdb quit result:", quit_res)
print("Bdb quit stopped before return:", ("return", "target_work", 16) not in rec_quit.events)

# Bdb breakpoints: conditional + ignore + clear
bdb.Breakpoint.clearBreakpoints()


def loop_fn():
    acc = 0
    for i in range(5):
        acc += i
    return acc


bp_line = loop_fn.__code__.co_firstlineno + 3  # `acc += i` line


class BpBdb(bdb.Bdb):
    def __init__(self):
        super().__init__()
        self.hits = []

    def do_clear(self, arg):
        self.clear_bpbynumber(arg)

    def user_line(self, frame):
        if self.break_here(frame) or frame.f_lineno == bp_line:
            self.hits.append(frame.f_locals.get("i"))
        self.set_continue()


bpd = BpBdb()
err = bpd.set_break(__file__, bp_line, cond="i >= 2")
print("set_break err:", err)
print("get_break:", bpd.get_break(__file__, bp_line))
print("get_breaks count:", len(bpd.get_breaks(__file__, bp_line)))
bp_obj = bpd.get_breaks(__file__, bp_line)[0]
bp_obj.ignore = 1  # skip i==2, stop on i==3 and i==4
bpd.reset()
bpd.set_continue()
# Use runcall starting with set_continue on first line
class BpRunnerBdb(bdb.Bdb):
    def __init__(self):
        super().__init__()
        self.seen = []
        self.started = False

    def do_clear(self, arg):
        self.clear_bpbynumber(arg)

    def user_line(self, frame):
        if not self.started:
            self.started = True
            self.set_continue()
            return
        self.seen.append(frame.f_locals.get("i"))
        self.set_continue()


runner = BpRunnerBdb()
print("loop_fn result:", runner.runcall(loop_fn))
print("conditional+ignore bp hits:", runner.seen)
print("clear_all_breaks err:", runner.clear_all_breaks())
print("get_all_breaks after clear:", runner.get_all_breaks())
bdb.Breakpoint.clearBreakpoints()


# ── 4. Scripted non-interactive Pdb session via io.StringIO ──────────────────
def inner_calc(x, y):
    z = x * y
    return z + 1


def outer_calc(a):
    b = inner_calc(a, 3)
    c = b + 10
    return c


script = "\n".join([
    "args",
    "p a",
    "whatis a",
    "whatis inner_calc",
    "s",
    "a",
    "n",
    "n",
    "p z",
    "pp {'k': z}",
    "u",
    "p a",
    "d",
    "p z",
    "r",
    "rv",
    "c",
]) + "\n"

out_buf = io.StringIO()
dbg = pdb.Pdb(stdin=io.StringIO(script), stdout=out_buf, readrc=False, nosigint=True)
pdb_res = dbg.runcall(outer_calc, 4)
print("pdb runcall result:", pdb_res)
pdb_out = norm(out_buf.getvalue())
print("pdb saw a = 4:", "a = 4" in pdb_out)
print("pdb saw x = 4:", "x = 4" in pdb_out)
print("pdb saw y = 3:", "y = 3" in pdb_out)
print("pdb saw whatis int:", "<class 'int'>" in pdb_out)
print("pdb saw whatis fn:", "Function inner_calc" in pdb_out)
print("pdb saw z = 12:", "(Pdb) 12\n" in pdb_out)
print("pdb saw pp dict:", "{'k': 12}" in pdb_out)
print("pdb saw --Return--:", "--Return--" in pdb_out)
print("pdb saw retval 13:", "(Pdb) 13\n" in pdb_out)

# Pdb breakpoints, disable/enable/clear, list, where, help, quit
bdb.Breakpoint.clearBreakpoints()
target_line = inner_calc.__code__.co_firstlineno + 1
script2 = "\n".join([
    f"b {target_line}",
    "b",
    "disable 1",
    "enable 1",
    "cl 1",
    "l",
    "w",
    "h step",
    "q",
]) + "\n"
out_buf2 = io.StringIO()
dbg2 = pdb.Pdb(stdin=io.StringIO(script2), stdout=out_buf2, readrc=False, nosigint=True)
quit_val = dbg2.runcall(outer_calc, 7)
print("pdb quit return:", quit_val)
pdb_out2 = norm(out_buf2.getvalue())
print("pdb break set:", f"Breakpoint 1 at <TESTFILE>:{target_line}" in pdb_out2)
print("pdb disabled:", "Disabled breakpoint 1" in pdb_out2)
print("pdb enabled:", "Enabled breakpoint 1" in pdb_out2)
print("pdb deleted:", "Deleted breakpoint 1" in pdb_out2)
print("pdb list arrow:", "->" in pdb_out2)
print("pdb help step:", "s(tep)" in pdb_out2)
bdb.Breakpoint.clearBreakpoints()

# Pdb post_mortem error when no exception active
try:
    pdb.post_mortem()
    print("ERROR: expected ValueError")
except ValueError as exc:
    print("post_mortem no-exc err:", str(exc))

# Pdb post-mortem with scripted Pdb.interaction on real traceback
pm_buf = io.StringIO()
pm_dbg = pdb.Pdb(stdin=io.StringIO("p msg\nw\nq\n"), stdout=pm_buf, readrc=False, nosigint=True)
try:
    def boom():
        msg = "kaboom"
        raise KeyError(msg)
    boom()
except KeyError as exc:
    pm_dbg.reset()
    pm_dbg.interaction(None, exc.__traceback__)
pm_out = norm(pm_buf.getvalue())
print("post_mortem saw msg:", "'kaboom'" in pm_out)
print("post_mortem saw boom:", "boom()" in pm_out)


# ── 5. Default sys.breakpointhook wired to pdb.set_trace ─────────────────────
orig_hook = sys.breakpointhook
orig_env = os.environ.get("PYTHONBREAKPOINT")
orig_set_trace = pdb.set_trace
hook_calls = []
try:
    def fake_set_trace(*args, **kwargs):
        hook_calls.append((args, sorted(kwargs.items())))
        return "traced"

    pdb.set_trace = fake_set_trace
    sys.breakpointhook = sys.__breakpointhook__
    if "PYTHONBREAKPOINT" in os.environ:
        del os.environ["PYTHONBREAKPOINT"]
    print("default breakpoint unset env:", breakpoint(header="hello"))
    os.environ["PYTHONBREAKPOINT"] = ""
    print("default breakpoint empty env:", breakpoint(header="empty"))
    os.environ["PYTHONBREAKPOINT"] = "pdb.set_trace"
    print("explicit pdb.set_trace env:", breakpoint(header="explicit"))
    print("hook_calls:", hook_calls)
finally:
    pdb.set_trace = orig_set_trace
    sys.breakpointhook = orig_hook
    if orig_env is None:
        if "PYTHONBREAKPOINT" in os.environ:
            del os.environ["PYTHONBREAKPOINT"]
    else:
        os.environ["PYTHONBREAKPOINT"] = orig_env
