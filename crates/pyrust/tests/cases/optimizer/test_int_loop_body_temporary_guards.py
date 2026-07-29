# A loop whose body computes a branch operand into a temporary (`if i % 2 != 0`)
# is entered with that temporary still unset, so the specialized int-loop copy
# (issue #2898) retires its entry guard instead of diverting every execution to
# the original stream.  The temporary's value is a per-iteration fact produced
# by already-guarded inputs, so retiring the guard must not change anything a
# program can observe: the results below pin the fast copy, its deopt edges, and
# the module namespace a mid-loop side exit publishes.
#
# Line checks are recorded as offsets from a marker raise so they survive edits
# elsewhere in this file, and they read `tb_lineno` rather than caret text.

namespace = globals()

try:
    raise RuntimeError("line marker")
except RuntimeError as marker:
    marker_line = marker.__traceback__.tb_lineno


# ── The shape the guard change unlocks, at module and function scope ──────────
mod_i = 0
mod_total = 0
while mod_i < 40:
    if mod_i % 2 != 0:
        mod_total += mod_i
    mod_i += 1
print("module inverted", mod_i, mod_total)


def function_inverted(limit):
    i = 0
    total = 0
    while i < limit:
        if i % 2 != 0:
            total += i
        i += 1
    return i, total


print("function inverted", function_inverted(40))


def function_continue(limit):
    i = 0
    total = 0
    while i < limit:
        if i % 3 == 0:
            i += 1
            continue
        total += i
        i += 1
    return i, total


print("function continue", function_continue(40))


# A zero-trip loop must leave the body temporary untouched and unbound.
def zero_trip():
    i = 10
    total = 0
    while i < 10:
        if i % 2 != 0:
            total += i
        i += 1
    return i, total


print("zero trip", zero_trip())


# One-trip and two-trip boundaries, both parities.
for limit in (0, 1, 2, 3):
    print("boundary", limit, function_inverted(limit))


# ── The entry state that still has to deopt ──────────────────────────────────
def non_int_bound(limit):
    i = 0
    total = 0
    while i < limit:
        if i % 2 != 0:
            total += i
        i += 1
    return i, total


print("float bound", non_int_bound(6.5))


def non_int_accumulator(start):
    i = 0
    total = start
    while i < 6:
        if i % 2 != 0:
            total += i
        i += 1
    return i, total


print("float accumulator", non_int_accumulator(0.5))


class Adds:
    """A user `__add__` reached from inside the loop the fast copy owns."""

    def __init__(self):
        self.seen = []

    def __add__(self, other):
        self.seen.append((other, namespace.get("probe_i"), namespace.get("probe_total") is self))
        return self


probe = Adds()
probe_i = 0
probe_total = probe
while probe_i < 6:
    if probe_i % 2 != 0:
        probe_total += probe_i
    probe_i += 1
print("radd seen", probe.seen)
print("radd final", probe_i, probe_total is probe)


# A big accumulator promotes to arbitrary precision mid-loop.
def promotes():
    i = 0
    total = (1 << 62) - 3
    while i < 6:
        if i % 2 != 0:
            total += i
        i += 1
    return i, total


print("promotes", promotes())


# ── A raise from the body reports the original loop's own line ───────────────
def raising(values):
    i = 0
    total = 0
    while i < len(values):
        if i % 2 != 0:
            total += values[i]
        i += 1
    return total


try:
    raising([1, "boom", 4, 8])
except TypeError as error:
    frame = error.__traceback__
    while frame.tb_next is not None:
        frame = frame.tb_next
    print("TypeError at marker +", frame.tb_lineno - marker_line)


# The loop variable a `finally` observes after a break out of the fast copy.
trace = []
break_i = 0
break_total = 0
try:
    while break_i < 40:
        if break_i % 2 != 0:
            break_total += break_i
            if break_total > 10:
                break
        break_i += 1
finally:
    trace.append((namespace["break_i"], namespace["break_total"]))
print("break", trace)


# ── The same temporary shape over a `for` loop ───────────────────────────────
def for_inverted(limit):
    total = 0
    for i in range(limit):
        if i % 2 != 0:
            total += i
    return total


print("for inverted", [for_inverted(limit) for limit in (0, 1, 2, 40)])
