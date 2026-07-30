# `pass_inline_leaf_binop` prefixes an eligible two-argument leaf call with a
# `CallInlineBinOp` guard whose success path computes `a <op> b` inline and
# skips the call sequence entirely.  The original `Move ×3 + Call` sequence is
# left in place byte-for-byte as the deopt path.
#
# The guard holds only when the callee is still the exact code object the proto
# was compiled from *and* both arguments are machine ints, so everything below
# is a way for one of those to stop being true mid-run.  Whenever it does, the
# real call must happen — with its own frame, its own dunder dispatch, and its
# own arbitrary-precision arithmetic.
#
# Two shapes are load-bearing for the guard actually being *there*, and both are
# deliberate throughout this file:
#
#   * each mutated callee has its own name, bound by exactly one `def`, so the
#     runtime rebinding a section performs is the only thing that can move the
#     binding out from under its guard.  A second `def` of the same name would
#     open a fresh region wired to its own proto (see
#     test_call_inline_binop_rebound_regions.py) and the site being probed here
#     would no longer be the one the section rebinds.
#   * both arguments are registers that already hold their value at the call.
#     A literal spelled at the call site is materialised inside the sequence,
#     which is a different (and here, frequently unmatched) site shape.
#
# Keep both when adding a case, or the new section will pin ordinary call
# semantics rather than the guard.

import sys

ONE = 1
TWO = 2
THOUSAND = 1000
BIG = 1 << 70
HALF = 0.5


def add(a, b):
    return a + b


def rsub(a, b):
    return b - a


def rmul(a, b):
    return b * a


# ── The eligible shapes themselves ───────────────────────────────────────────
total = 0
for i in range(200):
    total += add(i, ONE)
print("add loop", total)

swapped = 0
for i in range(200):
    swapped += rsub(i, THOUSAND)
print("swapped sub loop", swapped)

swapped_mul = 1
for i in range(1, 20):
    swapped_mul = rmul(swapped_mul, ONE) + rmul(ONE, i)
print("swapped mul loop", swapped_mul)

print("literal call", add(1073741824, 1073741825), rsub(4, 11), rmul(3, 7))


# ── Rebinding the callee mid-loop by plain assignment ────────────────────────
def times_hundred(a, b):
    return a * 100


def add_rebound(a, b):
    return a + b


plain = []
for i in range(6):
    plain.append(add_rebound(i, ONE))
    if i == 2:
        add_rebound = times_hundred
print("plain rebind", plain)


# Rebinding through `exec`, which writes the module namespace behind the
# compiler's back: the guard's code-object identity check is the only thing
# standing between this and a wrong answer.
def add_execed(a, b):
    return a + b


execed = []
for i in range(6):
    execed.append(add_execed(i, ONE))
    if i == 2:
        exec("def add_execed(a, b):\n    return a - b\n")
print("exec rebind", execed)


# Rebinding to a *different* function with an identically eligible body still
# has a different code object, so it deopts at the old site.
def add_retargeted(a, b):
    return a + b


def add_again(a, b):
    return a + b


retargeted = []
for i in range(6):
    retargeted.append(add_retargeted(i, ONE))
    if i == 2:
        add_retargeted = add_again
print("retargeted", retargeted, add_retargeted is add_again)


# An alias to the same function keeps the same code object.
def add_aliased(a, b):
    return a + b


alias = add_aliased
aliased = 0
for i in range(50):
    aliased += alias(i, TWO)
print("alias", aliased)


# Rebinding to something that is not a function at all.
def add_noncallable(a, b):
    return a + b


noncallable = []
try:
    for i in range(6):
        noncallable.append(add_noncallable(i, ONE))
        if i == 2:
            add_noncallable = 5
except TypeError:
    print("non-callable rebind", noncallable, "TypeError")


# A callable instance is a different callee kind, not a `Regular` user function.
class Adder:
    def __call__(self, a, b):
        return a + b + 1000


def add_instance(a, b):
    return a + b


instance_called = 0
for i in range(20):
    instance_called += add_instance(i, ONE)
    if i == 9:
        add_instance = Adder()
print("callable instance", instance_called)


# ── Argument types that fail the machine-int half of the guard ───────────────
print("bool args", add(True, ONE), add(True, False), rmul(True, 3), type(add(True, ONE)).__name__)
print("bigint args", add(BIG, ONE), add(ONE, BIG), rsub(BIG, 1 << 71))
print("overflowing add", add((1 << 62) * 3, (1 << 62) * 3))
print("overflowing mul", rmul(1 << 40, 1 << 40))
print("i64 max", add((1 << 63) - 1, ONE), rsub(ONE, -(1 << 63)))
print("float args", add(HALF, ONE), add(ONE, HALF), rsub(HALF, 2.5))
print("str args", add("a", "b"), rmul("ab", 3))
print("seq args", add([1], [2]), add((1,), (2,)), rmul((7,), 2))


# `bool` is never the exact int the guard admits, so this guarded site deopts on
# every one of its iterations.
def add_bool(a, b):
    return a + b


bool_total = 0
for i in range(20):
    flag = i % 2 == 0
    bool_total += add_bool(flag, i)
print("bool loop", bool_total)


# The sections below alternate eligible and ineligible arguments through one
# guarded site, so the guard is re-entered on the success path after every
# deopt rather than failing once and staying failed.


def add_bigint(a, b):
    return a + b


big_total = 0
for i in range(20):
    left = BIG if i % 2 else i
    big_total += add_bigint(left, i)
print("bigint loop", big_total - 10 * BIG)


def add_mixed(a, b):
    return a + b


mixed_total = 0
for i in range(20):
    right = HALF if i % 3 == 0 else ONE
    mixed_total += add_mixed(i, right)
print("mixed loop", mixed_total, type(mixed_total).__name__)


def add_overflowing(a, b):
    return a + b


# `near_max` is a machine int, so the guard admits both arguments; the *result*
# is what leaves i64 half-way through, which the inline arithmetic must decline
# rather than wrap.
overflow_total = 0
near_max = (1 << 63) - 5
for i in range(10):
    overflow_total += add_overflowing(near_max, i) - near_max
print("overflow loop", overflow_total, type(overflow_total).__name__)


# ── User protocol code reached through the deopt path ────────────────────────
class Probe:
    def __init__(self, label):
        self.label = label

    def __radd__(self, other):
        return "%s+%s" % (other, self.label)

    def __add__(self, other):
        return "%s+%s" % (self.label, other)

    def __rsub__(self, other):
        return "%s-%s" % (other, self.label)


def add_protocol(a, b):
    return a + b


probe = Probe("p")
protocol = []
for i in range(6):
    right = probe if i % 2 else ONE
    protocol.append(add_protocol(i, right))
print("protocol loop", protocol)
print("protocol", add(1, probe), add(probe, 1), rsub(probe, 2))


class FrameProbe:
    """Reports the call stack it is invoked from, so an elided frame is visible."""

    def __radd__(self, other):
        names = []
        depth = 0
        while True:
            try:
                names.append(sys._getframe(depth).f_code.co_name)
            except ValueError:
                break
            depth += 1
        return names


def add_frames(a, b):
    return a + b


frame_probe = FrameProbe()
module_frames = None
for i in range(6):
    right = frame_probe if i == 3 else ONE
    module_frames = add_frames(ONE, right)
print("module frames", module_frames)


def driver():
    def leaf(a, b):
        return a + b

    seen = None
    one = 1
    for i in range(20):
        right = frame_probe if i == 10 else one
        seen = leaf(1, right)
    return seen


print("function frames", driver())


def nested_driver():
    def leaf(a, b):
        return a + b

    def inner():
        one = 1
        seen = None
        for i in range(4):
            right = frame_probe if i == 2 else one
            seen = leaf(1, right)
        return seen

    return inner()


print("nested frames", nested_driver())


# ── Prototypes the pass must never consider eligible ─────────────────────────
def default_leaf(a, b=10):
    return a + b


def kwonly_leaf(a, *, b):
    return a + b


def star_leaf(*args):
    return args[0] + args[1]


def kwargs_leaf(a, b, **rest):
    return a + b


def three_leaf(a, b, c=0):
    return a + b


def divide_leaf(a, b):
    return a // b


def global_leaf(a, b):
    global module_marker
    module_marker = a
    return a + b


def closure_leaf(a, b):
    def _inner():
        return a

    return a + b


def generator_leaf(a, b):
    yield a + b


module_marker = 0
print("default", default_leaf(1), default_leaf(1, 2), default_leaf(1, b=3))
print("kwonly", kwonly_leaf(1, b=2))
print("star", star_leaf(1, 2), kwargs_leaf(1, 2, extra=3))
print("three", three_leaf(1, 2), three_leaf(1, 2, 3))
print("divide", divide_leaf(7, 2), divide_leaf(-7, 2))
print("global", global_leaf(4, 5), module_marker)
print("closure", closure_leaf(4, 5))
print("generator", list(generator_leaf(4, 5)))

default_total = 0
for i in range(50):
    default_total += default_leaf(i)
print("default loop", default_total)

global_total = 0
for i in range(50):
    global_total += global_leaf(i, ONE)
print("global loop", global_total, module_marker)

closure_total = 0
for i in range(50):
    closure_total += closure_leaf(i, ONE)
print("closure loop", closure_total)

divide_total = 0
for i in range(1, 50):
    divide_total += divide_leaf(i, TWO)
print("divide loop", divide_total)

keyword_total = 0
for i in range(50):
    keyword_total += add(a=i, b=ONE)
print("keyword call loop", keyword_total)

splat_total = 0
pair = (1, 2)
for i in range(50):
    splat_total += add(*pair)
print("splat call loop", splat_total)


# ── Errors raised by the deopted call keep the callee's own frame ────────────
try:
    divide_leaf(1, 0)
except ZeroDivisionError as error:
    frames = []
    frame = error.__traceback__
    while frame is not None:
        frames.append(frame.tb_frame.f_code.co_name)
        frame = frame.tb_next
    print("ZeroDivisionError frames", frames, str(error))


def add_raising(a, b):
    return a + b


text = "not an int"
try:
    for i in range(4):
        right = text if i == 2 else ONE
        add_raising(i, right)
except TypeError as error:
    frames = []
    frame = error.__traceback__
    while frame is not None:
        frames.append(frame.tb_frame.f_code.co_name)
        frame = frame.tb_next
    print("TypeError frames", frames)

try:
    add(1)
except TypeError:
    print("arity", "TypeError")
