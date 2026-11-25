# CPython 3.12 parity tests for the `atexit` module (issue #3209).

import atexit
import contextlib
import io

atexit._clear()
print("initial after clear:", atexit._ncallbacks())

# ── LIFO execution order, args, kwargs, and decorator form ───────────────────
events = []


@atexit.register
def first():
    events.append("first")


def second(a, b, c=0):
    events.append(("second", a, b, c))


ret = atexit.register(second, 1, 2, c=3)
print("register returns func:", ret is second)
print("ncallbacks after 2:", atexit._ncallbacks())

atexit._run_exitfuncs()
print("events LIFO:", events)
print("ncallbacks after run:", atexit._ncallbacks())

# ── unregister removes all equal occurrences (including custom __eq__) ───────
events.clear()


def target(tag):
    events.append(("target", tag))


def keep(tag):
    events.append(("keep", tag))


atexit.register(target, 1)
atexit.register(keep, "a")
atexit.register(target, 2)
atexit.register(keep, "b")
print("before unregister:", atexit._ncallbacks())
atexit.unregister(target)
print("after unregister:", atexit._ncallbacks())
atexit._run_exitfuncs()
print("events after unregister:", events)
print("after run:", atexit._ncallbacks())

# Equality-based unregister
events.clear()


class EqCallback:
    def __init__(self, key, label):
        self.key = key
        self.label = label

    def __eq__(self, other):
        return isinstance(other, EqCallback) and self.key == other.key

    def __call__(self):
        events.append(self.label)


atexit.register(EqCallback(1, "one-a"))
atexit.register(EqCallback(2, "two"))
atexit.register(EqCallback(1, "one-b"))
atexit.unregister(EqCallback(1, "probe"))
atexit._run_exitfuncs()
print("eq unregister events:", events)

# unregister of non-callable / missing object is a no-op
atexit.unregister(12345)
atexit.unregister("missing")
print("unregister missing ok")

# ── Error handling on bad arguments ──────────────────────────────────────────
for label, call in [
    ("register()", lambda: atexit.register()),
    ("register(func=keep)", lambda: atexit.register(func=keep)),
    ("register(123)", lambda: atexit.register(123)),
    ("unregister()", lambda: atexit.unregister()),
    ("_clear(1)", lambda: atexit._clear(1)),
    ("_ncallbacks(1)", lambda: atexit._ncallbacks(1)),
    ("_run_exitfuncs(1)", lambda: atexit._run_exitfuncs(1)),
]:
    try:
        call()
    except TypeError:
        print(label, "TypeError")

# ── Exceptions inside exit callbacks do not abort subsequent callbacks ───────
events.clear()


def good_first():
    events.append("good_first")


def bad_middle():
    events.append("bad_middle")
    raise ValueError("boom-atexit")


def bad_sysexit():
    events.append("bad_sysexit")
    raise SystemExit(7)


def good_last():
    events.append("good_last")


atexit.register(good_first)
atexit.register(bad_middle)
atexit.register(bad_sysexit)
atexit.register(good_last)

buf = io.StringIO()
with contextlib.redirect_stderr(buf):
    atexit._run_exitfuncs()

err_out = buf.getvalue()
print("events with errors:", events)
print("ncallbacks after errors:", atexit._ncallbacks())
print("saw ignored header:", "Exception ignored in atexit callback:" in err_out)
print("saw ValueError:", "ValueError: boom-atexit" in err_out)
print("saw SystemExit:", "SystemExit: 7" in err_out)

# ── Re-registering during _run_exitfuncs ─────────────────────────────────────
events.clear()


def outer():
    events.append("outer")
    atexit.register(lambda: events.append("inner"))


atexit.register(outer)
atexit._run_exitfuncs()
print("reregister events:", events, "ncallbacks:", atexit._ncallbacks())

# ── Real interpreter shutdown LIFO execution ─────────────────────────────────
atexit.register(print, "shutdown-1")
atexit.register(print, "shutdown-2")
