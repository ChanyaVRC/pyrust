# A mid-loop side exit out of the specialized int-loop copy (PR #2895, issue
# #2887) flushes every deferred `SyncModuleGlobal` before jumping back into the
# original loop.  Whatever runs next — user protocol code, an exception handler,
# a `finally`, a traceback — must therefore see a module namespace that is
# indistinguishable from the one the unversioned loop would have published, and
# a raise must report the original loop's own line.
#
# Line checks are recorded as offsets from a marker raise so they survive edits
# elsewhere in this file, and they read `tb_lineno` rather than caret text.

import sys

namespace = globals()

try:
    raise RuntimeError("line marker")
except RuntimeError as marker:
    marker_line = marker.__traceback__.tb_lineno


# ── A user `__radd__` fired mid-loop reads the live module namespace ──────────
class Probe:
    """Reflected add: runs Python code from inside a loop the fast copy owns."""

    def __init__(self, label):
        self.label = label

    def __radd__(self, other):
        observed.append(
            (
                self.label,
                other,
                namespace["radd_total"],
                namespace["radd_index"],
                namespace["radd_value"] is self,
            )
        )
        return other


observed = []
radd_total = 0
radd_index = 0
for radd_value in [1, 2, Probe("first"), 8, Probe("second"), 32]:
    radd_index += 1
    radd_total += radd_value
print("radd observed", observed)
print("radd final", radd_total, radd_index)

# The same probe reached through the fast subscript rather than the loop target.
observed = []
radd_total = 0
radd_index = 0
radd_value = None
sub_source = [1, 2, Probe("sub"), 8]
for sub_index in range(4):
    radd_index += 1
    radd_total += sub_source[sub_index]
print("subscript radd observed", observed)
print("subscript radd final", radd_total, radd_index, sub_index)


# A `__radd__` that itself walks the whole namespace snapshot must see every
# name the loop has bound so far, and nothing it has not.
class NameProbe:
    def __radd__(self, other):
        print(
            "nameprobe",
            other,
            "walk_total" in namespace,
            namespace.get("walk_total"),
            "walk_after" in namespace,
        )
        return other


walk_total = 0
for walk_value in [1, 2, NameProbe(), 8]:
    walk_total += walk_value
walk_after = walk_total
print("nameprobe final", walk_total, walk_after)


# ── try / except / finally wrapped around a versioned loop at module scope ────
trace = []
try:
    clean_total = 0
    for clean_value in [1, 2, 4, 8]:
        clean_total += clean_value
    trace.append(("body", namespace["clean_total"], namespace["clean_value"]))
    raise ValueError("after the loop")
except ValueError as error:
    trace.append(("except", str(error), namespace["clean_total"]))
finally:
    trace.append(("finally", namespace["clean_total"], namespace["clean_value"]))
print("clean", trace)

# The loop itself raises, after a side exit has already fired.
trace = []
raising_total = 0
raising_seen = []
try:
    for raising_value in [1, 2, "deopt", 8]:
        raising_seen.append(raising_value)
        raising_total += raising_value
except TypeError as error:
    trace.append(("except", namespace["raising_total"], list(namespace["raising_seen"])))
finally:
    trace.append(("finally", namespace["raising_value"], namespace["raising_total"]))
print("raising", trace)

# `finally` runs on the way out of a `break`, with the loop variable published.
trace = []
try:
    for break_value in [1, 2, 4, 8]:
        if break_value == 4:
            break
finally:
    trace.append(("finally", namespace["break_value"]))
print("break", trace)

# `for … else` after a versioned loop that completed, and one that broke.
for else_value in [1, 2, 4]:
    pass
else:
    print("else ran", namespace["else_value"])

for else_break in [1, 2, 4]:
    if else_break == 2:
        break
else:
    print("unreachable")
print("else skipped", else_break)


# ── An IndexError raised mid-loop reports the original loop's line ────────────
index_source = [1, 2, 4]
index_total = 0
index_i = 0
try:
    for index_i in range(6):
        index_total += index_source[index_i]
except IndexError as error:
    frame = error.__traceback__
    depth = 0
    while frame.tb_next is not None:
        frame = frame.tb_next
        depth += 1
    print("IndexError depth", depth)
    print("IndexError at marker +", frame.tb_lineno - marker_line)
    print("IndexError globals", namespace["index_i"], namespace["index_total"])

# A TypeError raised by the deopted body reports its own line too.
type_total = 0
try:
    for type_value in [1, 2, "boom", 8]:
        type_total += type_value
except TypeError as error:
    frame = error.__traceback__
    while frame.tb_next is not None:
        frame = frame.tb_next
    print("TypeError at marker +", frame.tb_lineno - marker_line)
    print("TypeError globals", namespace["type_value"], namespace["type_total"])


# A raise from inside a function called by the loop keeps both frames, and the
# module frame's line is the call site inside the loop.
def blow_up(value):
    raise ZeroDivisionError("from %d" % value)


call_total = 0
try:
    for call_value in [1, 2, 4]:
        call_total += call_value
        if call_value == 2:
            blow_up(call_value)
except ZeroDivisionError as error:
    names = []
    frame = error.__traceback__
    while frame is not None:
        names.append((frame.tb_frame.f_code.co_name, frame.tb_lineno - marker_line))
        frame = frame.tb_next
    print("ZeroDivisionError frames", names)
    print("ZeroDivisionError globals", namespace["call_total"], namespace["call_value"])


# ── The frame the deopted body observes is the module frame ──────────────────
class FrameProbe:
    def __radd__(self, other):
        frame = sys._getframe(1)
        print("frameprobe", frame.f_code.co_name, namespace["frame_total"])
        return other


frame_total = 0
for frame_value in [1, 2, FrameProbe(), 8]:
    frame_total += frame_value
print("frameprobe final", frame_total)


def inside_a_function():
    total = 0
    for value in [1, 2, FrameProbe(), 8]:
        total += value
    return total


frame_total = -1
print("frameprobe function", inside_a_function())
