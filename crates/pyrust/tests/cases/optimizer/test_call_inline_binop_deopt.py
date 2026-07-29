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

import sys


def add(a, b):
    return a + b


def rsub(a, b):
    return b - a


def rmul(a, b):
    return b * a


# ── The eligible shapes themselves ───────────────────────────────────────────
total = 0
for i in range(200):
    total += add(i, 1)
print("add loop", total)

swapped = 0
for i in range(200):
    swapped += rsub(i, 1000)
print("swapped sub loop", swapped)

swapped_mul = 1
for i in range(1, 20):
    swapped_mul = rmul(swapped_mul, 1) + rmul(1, i)
print("swapped mul loop", swapped_mul)

print("literal call", add(1073741824, 1073741825), rsub(4, 11), rmul(3, 7))


# ── Rebinding the callee mid-loop by plain assignment ────────────────────────
def times_hundred(a, b):
    return a * 100


plain = []
for i in range(6):
    plain.append(add(i, 1))
    if i == 2:
        add = times_hundred
print("plain rebind", plain)


def add(a, b):
    return a + b


# Rebinding through `exec`, which writes the module namespace behind the
# compiler's back: the guard's code-object identity check is the only thing
# standing between this and a wrong answer.
execed = []
for i in range(6):
    execed.append(add(i, 1))
    if i == 2:
        exec("def add(a, b):\n    return a - b\n")
print("exec rebind", execed)


def add(a, b):
    return a + b


# Rebinding to a *different* function with an identically eligible body still
# has a different code object, so it deopts and then re-inlines at the new site.
def add_again(a, b):
    return a + b


retargeted = []
for i in range(6):
    retargeted.append(add(i, 1))
    if i == 2:
        add = add_again
print("retargeted", retargeted, add is add_again)

add = add_again

# An alias to the same function keeps the same code object.
alias = add
aliased = 0
for i in range(50):
    aliased += alias(i, 2)
print("alias", aliased)

# Rebinding to something that is not a function at all.
noncallable = []
try:
    for i in range(6):
        noncallable.append(add(i, 1))
        if i == 2:
            add = 5
except TypeError:
    print("non-callable rebind", noncallable, "TypeError")

add = add_again

# A callable instance is a different callee kind, not a `Regular` user function.
class Adder:
    def __call__(self, a, b):
        return a + b + 1000


add = Adder()
instance_called = 0
for i in range(20):
    instance_called += add(i, 1)
print("callable instance", instance_called)

add = add_again


# ── Argument types that fail the machine-int half of the guard ───────────────
print("bool args", add(True, 1), add(True, False), rmul(True, 3), type(add(True, 1)).__name__)
print("bigint args", add(1 << 70, 1), add(1, 1 << 70), rsub(1 << 70, 1 << 71))
print("overflowing add", add((1 << 62) * 3, (1 << 62) * 3))
print("overflowing mul", rmul(1 << 40, 1 << 40))
print("i64 max", add((1 << 63) - 1, 1), rsub(1, -(1 << 63)))
print("float args", add(0.5, 1), add(1, 0.5), rsub(0.5, 2.5))
print("str args", add("a", "b"), rmul("ab", 3))
print("seq args", add([1], [2]), add((1,), (2,)), rmul((7,), 2))

bool_total = 0
for i in range(20):
    bool_total += add(i % 2 == 0, i)
print("bool loop", bool_total)

big_total = 0
for i in range(20):
    big_total += add(1 << 70, i)
print("bigint loop", big_total - 20 * (1 << 70))

mixed_total = 0
for i in range(20):
    mixed_total += add(i, 0.5 if i % 3 == 0 else 1)
print("mixed loop", mixed_total, type(mixed_total).__name__)


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


print("protocol", add(1, Probe("p")), add(Probe("p"), 1), rsub(Probe("p"), 2))


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


print("module frames", add(1, FrameProbe()))


def driver():
    seen = None
    for _ in range(20):
        seen = add(1, FrameProbe())
    return seen


print("function frames", driver())


def nested_driver():
    def inner():
        return add(1, FrameProbe())

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

keyword_total = 0
for i in range(50):
    keyword_total += add(a=i, b=1)
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

try:
    for i in range(4):
        add(i, "not an int")
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
