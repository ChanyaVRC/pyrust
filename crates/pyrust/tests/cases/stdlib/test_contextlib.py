# Tests for the contextlib module: suppress, contextmanager, closing, nullcontext,
# ExitStack.  Each section exercises the happy path and the relevant error paths.

import contextlib

# ── suppress ──────────────────────────────────────────────────────────────────

# Normal exit: no exception — no-op
with contextlib.suppress(ValueError):
    pass
print("suppress no-exception ok")

# Single exception type — suppressed
with contextlib.suppress(ValueError):
    raise ValueError("suppressed")
print("suppress single type ok")

# Multiple exception types — suppressed
with contextlib.suppress(TypeError, ValueError):
    raise TypeError("also suppressed")
print("suppress multiple types ok")

# Subclass is also suppressed
class MyError(ValueError):
    pass

with contextlib.suppress(ValueError):
    raise MyError("subclass suppressed")
print("suppress subclass ok")

# Non-matching exception propagates
try:
    with contextlib.suppress(ValueError):
        raise RuntimeError("not suppressed")
except RuntimeError:
    print("suppress non-match propagates ok")

# Base class is NOT suppressed when only subclass is listed
try:
    with contextlib.suppress(MyError):
        raise ValueError("base not suppressed")
except ValueError:
    print("suppress base not suppressed ok")

# ── contextmanager ────────────────────────────────────────────────────────────

# Basic usage: yield a value, code after yield runs on normal exit
@contextlib.contextmanager
def cm_basic():
    print("cm enter")
    yield 99
    print("cm exit")

with cm_basic() as v:
    print(f"cm value={v}")
print("cm after")

# Exception propagates when not handled
@contextlib.contextmanager
def cm_reraise():
    yield

try:
    with cm_reraise():
        raise ValueError("propagated")
except ValueError as e:
    print(f"cm propagated: {e}")

# Generator catches and swallows exception
@contextlib.contextmanager
def cm_swallow():
    try:
        yield
    except RuntimeError:
        print("cm caught RuntimeError")

with cm_swallow():
    raise RuntimeError("swallowed")
print("cm swallow ok")

# ── closing ───────────────────────────────────────────────────────────────────

class Closeable:
    def close(self):
        print("Closeable.close()")

c = Closeable()
with contextlib.closing(c) as obj:
    print(f"closing: obj is c = {obj is c}")
print("closing after")

# close() is called even if exception occurs
class Closeable2:
    def close(self):
        print("Closeable2.close()")

c2 = Closeable2()
try:
    with contextlib.closing(c2):
        raise ValueError("closing with exception")
except ValueError:
    pass
print("closing with exception: close was called above")

# ── nullcontext ───────────────────────────────────────────────────────────────

# With explicit enter_result
with contextlib.nullcontext(42) as v:
    print(f"nullcontext(42) = {v}")

# Default is None
with contextlib.nullcontext() as v:
    print(f"nullcontext() = {v}")

# Exception propagates through nullcontext
try:
    with contextlib.nullcontext():
        raise ValueError("nullcontext propagate")
except ValueError as e:
    print(f"nullcontext propagate: {e}")

# ── ExitStack ─────────────────────────────────────────────────────────────────

# Basic usage as context manager
with contextlib.ExitStack() as stack:
    print("ExitStack entered")
print("ExitStack exited")

# enter_context registers a context manager
log = []
class Recorder:
    def __init__(self, name):
        self.name = name
    def __enter__(self):
        log.append(f"enter {self.name}")
        return self
    def __exit__(self, *args):
        log.append(f"exit {self.name}")
        return False

with contextlib.ExitStack() as stack:
    stack.enter_context(Recorder("A"))
    stack.enter_context(Recorder("B"))
print(log)  # enter A, enter B, exit B, exit A (LIFO)

# callback is called on exit
log2 = []
with contextlib.ExitStack() as stack:
    stack.callback(log2.append, "cb1")
    stack.callback(log2.append, "cb2")
print(log2)  # cb2, cb1 (LIFO)

# ExitStack.close() outside with block
stack = contextlib.ExitStack()
log3 = []
stack.callback(log3.append, "x")
stack.close()
print(log3)  # ['x']

# suppress via enter_context
with contextlib.ExitStack() as stack:
    stack.enter_context(contextlib.suppress(ValueError))
    raise ValueError("suppressed by stack")
print("ExitStack suppressed ok")
