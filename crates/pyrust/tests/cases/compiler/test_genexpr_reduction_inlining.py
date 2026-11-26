# Pins the guarded lowering of `sum` / `any` / `all` / `min` / `max` applied
# directly to a generator expression (#3384): the reduction runs inside the
# `<genexpr>` frame when the callee value is the canonical builtin, and falls
# back to the ordinary generator + call when the name is shadowed or patched.
#
# Both sides are pinned: the fast path's results, laziness, exceptions and
# traceback frames, and every deopt edge (local / parameter / closure / module
# global / `builtins` rebinding and deletion).
import builtins
import sys
import traceback
from decimal import Decimal
from fractions import Fraction

xs = [3, 1, 4, 1, 5, 9, 2, 6]

print("== sum")
print(sum(x for x in xs))
print(sum(x for x in []))
print(sum(x * y for x in xs if x > 2 for y in (1, 2) if y != x))
print(sum(i * v for i, v in enumerate(xs)))
print(sum(a * b for a, b in zip(xs, reversed(xs))))
print(sum(i for i in range(1000)))
print(sum(1 for _ in enumerate(xs)))
print(sum(x > 2 for x in xs), sum(True for _ in xs), sum(False for _ in xs))
print(sum(x for x in [2**62, 2**62, 2**62]), sum(x for x in [2**70, -(2**70), 5]))
print(sum(len(s) for s in ["ab", "", "cde"]))

print("== sum start")
print(sum((x for x in xs), 100), sum((x for x in []), 7))
print(sum(([x] for x in xs[:3]), []), sum(((x,) for x in xs[:3]), ()))
print(sum((x for x in xs), 0.5), sum((x for x in [True, False]), True))
start = [0]
total = sum(([x] for x in (1, 2)), start)
print(total, start, total is start)
print(sum((x for x in []), start) is start)
print(sum((x for x in xs), start=10), sum((x for x in xs), -(2**70)))

print("== sum floats")
print(sum(0.1 for _ in range(10)), sum(0.1 for _ in range(10)) == 1.0)
print(sum((0.1 for _ in range(10)), 0.0), sum((0.1 for _ in range(10)), 0))
print(sum(x for x in [1, 0.1, 0.1, 0.1]), sum(x for x in [0.1, 1, 0.1, True, 0.1]))
print(sum(x for x in [2**70, 0.1, 0.1, 0.1]), sum(x for x in [0.1, 0.1, 2**70, 0.1]))
print(sum(x for x in [1e100, 1.0, -1e100]), sum(x for x in [1e308, 1e308, -1e308]))
print(sum(x for x in [float("inf"), 1.0]), sum(x for x in [float("inf"), float("-inf")]))
print(repr(sum((x for x in [-0.0]), -0.0)), repr(sum((x for x in [-0.0, -0.0]), -0.0)))
print(repr(sum(x for x in [-0.0])), repr(sum((x for x in []), -0.0)))
print(repr(sum([-0.0], -0.0)), repr(sum([0.1] * 10)), repr(sum([1, 0.1, 0.1, 0.1])))
print(sum(x for x in [0.1, 0.2, 1j]), sum(x for x in [0.1, 0.2, Fraction(1, 2), 0.3]))
print(sum(Fraction(1, n) for n in range(1, 6)), sum(Decimal(n) / 4 for n in range(4)))


class Money:
    def __init__(self, cents):
        self.cents = cents

    def __add__(self, other):
        return Money(self.cents + other.cents)

    def __radd__(self, other):
        return Money(self.cents + other)

    def __repr__(self):
        return f"Money({self.cents})"


print(sum(Money(c) for c in (5, 10, 20)), sum((Money(c) for c in (1, 2)), Money(100)))

print("== any / all")
print(any(x == 9 for x in xs), any(x == 10 for x in xs), any(x for x in []))
print(all(x >= 1 for x in xs), all(x > 1 for x in xs), all(x for x in []))
print(any(s for s in ["", [], 0, None, 0.0, ()]), any(s for s in ["", [1]]))
print(all(s for s in ["a", [1], 1, 2.5]), all(s for s in ["a", [], 1]))
print(any(x % 2 == 0 for x in xs if x > 5), all(x < 10 for row in [xs, xs] for x in row))


class Truthy:
    def __init__(self, value, log):
        self.value, self.log = value, log

    def __bool__(self):
        self.log.append(self.value)
        return self.value > 1


class Sized:
    def __init__(self, n):
        self.n = n

    def __len__(self):
        return self.n


log = []
print(any(Truthy(v, log) for v in (0, 1, 2, 3)), log)
log.clear()
print(all(Truthy(v, log) for v in (3, 2, 1, 0)), log)
print(any(Sized(n) for n in (0, 0)), all(Sized(n) for n in (1, 2)), all(Sized(n) for n in (1, 0)))

print("== short-circuit")
seen = []


def note(x):
    seen.append(x)
    return x


print(any(note(x) > 3 for x in xs), seen)
seen.clear()
print(all(note(x) < 4 for x in xs), seen)
seen.clear()
it = iter(xs)
print(any(v == 4 for v in it), list(it))
it = iter(xs)
print(all(v != 4 for v in it), list(it))

print("== min / max")
print(min(x * x for x in xs), max(x * x for x in xs))
print(min(x for x in [7]), max(x for x in [7]))
print(min((x for x in []), default="d"), max((x for x in []), default=None))
print(min((x for x in xs), default="d"), max((x for x in xs), default=None))
print(min(s for s in ["b", "a", "c"]), max(s for s in ["b", "a", "c"]))
print(min(x for x in [1.5, 1, 2]), max(x for x in [1, 1.0, True]), min(x for x in [1.0, 1, True]))
print(max(len(w) for w in "the quick brown fox".split()), min(abs(x - 4.5) for x in xs))
print(min((x, -x) for x in xs), max([x, -x] for x in xs))
print(max((x for x in xs), key=lambda v: -v), min((x for x in []), key=abs, default="k"))
nan = float("nan")
print(min(x for x in [nan, 1.0, 2.0]), min(x for x in [1.0, nan, 0.5]), max(x for x in [2.0, nan]))


class K:
    def __init__(self, k, tag):
        self.k, self.tag = k, tag

    def __lt__(self, other):
        return self.k < other.k

    def __gt__(self, other):
        return self.k > other.k

    def __repr__(self):
        return f"K({self.k},{self.tag})"


ks = [K(1, "a"), K(0, "b"), K(1, "c"), K(0, "d")]
print(min(k for k in ks), max(k for k in ks))


class Odd:
    # A comparison whose result is neither a bool nor always truthy.
    def __init__(self, v):
        self.v = v

    def __lt__(self, other):
        return [] if self.v >= other.v else [self.v]

    def __gt__(self, other):
        return "" if self.v <= other.v else "yes"

    def __repr__(self):
        return f"Odd({self.v})"


print(min(Odd(v) for v in (2, 1, 3)), max(Odd(v) for v in (2, 1, 3)))


class OnlyLt:
    # `max` asks `item > best`; with no `__gt__` that reflects to `__lt__`.
    def __init__(self, v):
        self.v = v

    def __lt__(self, other):
        return self.v < other.v

    def __repr__(self):
        return f"OnlyLt({self.v})"


print(min(OnlyLt(v) for v in (2, 1, 3)), max(OnlyLt(v) for v in (2, 1, 3)))

print("== errors")
for label, thunk in [
    ("min empty", lambda: min(x for x in [])),
    ("max empty", lambda: max(x for x in [])),
    ("max empty filtered", lambda: max(x for x in xs if x > 100)),
    ("sum str start", lambda: sum((x for x in ["a"]), "")),
    ("sum bytes start", lambda: sum((x for x in [b"a"]), b"")),
    ("sum bytearray start", lambda: sum((x for x in [b"a"]), bytearray())),
    ("sum str items", lambda: sum(x for x in ["a", "b"])),
    ("sum mixed", lambda: sum(x for x in [1, "a"])),
    ("sum float mixed", lambda: sum(x for x in [0.5, "a"])),
    ("sum none start", lambda: sum((x for x in [1]), None)),
    ("sum huge int float", lambda: sum(x for x in [0.5, 10**400])),
    ("max mixed", lambda: max(x for x in [1, "a"])),
    ("min mixed", lambda: min(x for x in ["a", 1])),
    ("body zero div", lambda: sum(1 // x for x in [1, 0])),
    ("cond zero div", lambda: any(x for x in [1, 0] if 1 // x)),
    ("not iterable", lambda: sum(x for x in 5)),
    ("inner not iterable", lambda: all(y for x in [1] for y in x)),
]:
    try:
        print(label, thunk())
    except Exception as exc:
        print(label, type(exc).__name__, exc)

print("== evaluation order")
seen.clear()
try:
    sum((note(x) for x in xs), "")
except TypeError as exc:
    print("start checked first", seen, exc)
seen.clear()
try:
    sum(note(x) for x in [1, "a", 2, 3])
except TypeError as exc:
    print("lazy failure", seen, exc)
seen.clear()
try:
    max(note(x) for x in [1, "a", 2, 3])
except TypeError as exc:
    print("lazy failure", seen, exc)
seen.clear()
print(sum((note(x) for x in note([1, 2])), note(10)), seen)
seen.clear()
print(min((note(x) for x in note([])), default=note("d")), seen)
seen.clear()
print(max((note(x) for x in note([4, 8])), default=note("d")), seen)


def rebinding_start():
    data = [1, 2, 3]
    # The generator captured `data` before the start expression rebinds it.
    return sum((v for v in data), (data := 100)), data


print(rebinding_start())


class Tracker:
    # Interleaving of element production and the builtin's own `+`.
    def __init__(self, n):
        self.n = n

    def __radd__(self, other):
        seen.append(("radd", self.n))
        return other + self.n


seen.clear()
print(sum(Tracker(note(n)) for n in (1, 2, 3)), seen)

print("== traceback frames")


class Bad:
    def __radd__(self, other):
        raise ValueError("bad radd")

    def __bool__(self):
        raise KeyError("bad bool")

    def __lt__(self, other):
        raise IndexError("bad lt")

    def __gt__(self, other):
        raise LookupError("bad gt")


class Stop:
    def __radd__(self, other):
        raise StopIteration("from radd")


class Interrupt(BaseException):
    pass


class Quit:
    def __bool__(self):
        raise Interrupt("from bool")


def frames(exc):
    out = []
    tb = exc.__traceback__
    while tb is not None:
        out.append(tb.tb_frame.f_code.co_name)
        tb = tb.tb_next
    return out


def run_sum(data):
    return sum(x for x in data)


def run_sum_start(data):
    return sum((x for x in data), 1.5)


def run_any(data):
    return any(x for x in data)


def run_all(data):
    return all(x for x in data)


def run_min(data):
    return min(x for x in data)


def run_max(data):
    return max(
        x
        for x in data
    )


def run_body(data):
    return sum(
        10 // x
        for x in data
        if x is not None
    )


def run_cond(data):
    return any(
        x
        for x in data
        if 10 // x
    )


def run_stop(it):
    return sum(next(it) for _ in range(5))


def run_nested_stop(it):
    return max(sum(next(it) for _ in range(2)) for _ in range(3))


for fn, data in [
    (run_sum, [1, "a"]),
    (run_sum, [1, Bad()]),
    (run_sum, [0.5, Bad()]),
    (run_sum, [1, Stop()]),
    (run_sum_start, [1, "a"]),
    (run_any, [0, Bad()]),
    (run_all, [1, Bad()]),
    (run_all, [1, Quit()]),
    (run_min, [Bad(), Bad()]),
    (run_max, [Bad(), Bad()]),
    (run_min, [1, "a"]),
    (run_max, [1, None]),
    (run_body, [1, 0]),
    (run_cond, [1, 0]),
    (run_stop, iter([1, 2])),
    (run_nested_stop, iter([1, 2, 3])),
]:
    try:
        print(fn.__name__, fn(data))
    except BaseException as exc:
        print(fn.__name__, type(exc).__name__, exc, frames(exc))
        print("  cause:", repr(exc.__cause__), "context:", repr(exc.__context__))

# The exception a reduction step raises is the very object the user raised.
marker = ValueError("identity")


class Raises:
    def __radd__(self, other):
        raise marker


try:
    sum(x for x in [1, Raises()])
except ValueError as exc:
    print(exc is marker, frames(exc))

# A step failing while another exception is being handled keeps its context.
try:
    try:
        raise RuntimeError("outer")
    except RuntimeError:
        sum(x for x in [1, "a"])
except TypeError as exc:
    print(type(exc).__name__, repr(exc.__context__), exc.__suppress_context__)

# Caught in the very frame that made the call, the calling frame is still the
# head of the traceback.
def catch_here(fn, data):
    try:
        return sum(x for x in data) if fn is sum else fn(x for x in data)
    except Exception as exc:
        return type(exc).__name__, frames(exc)


print(catch_here(sum, [1, Bad()]), catch_here(sum, [1, "a"]))
print(catch_here(min, [Bad(), Bad()]), catch_here(any, [Bad()]))
try:
    sum(x for x in [1, Bad()])
except ValueError as exc:
    print(frames(exc))
    traceback.print_exc(file=sys.stdout)

# The reduction survives being re-run after a failure.
for data in ([1, "a"], [1, 2], [1, Bad()], [3, 4]):
    try:
        print(run_sum(data), run_min(data))
    except Exception as exc:
        print(type(exc).__name__)

try:
    run_body([5, 0])
except ZeroDivisionError:
    traceback.print_exc(file=sys.stdout)
try:
    run_max([1, "a"])
except TypeError:
    traceback.print_exc(file=sys.stdout)
try:
    run_sum([1, Bad()])
except ValueError:
    traceback.print_exc(file=sys.stdout)

print("== shadowing and rebinding")


def my_sum(it, *rest):
    return ("my_sum", list(it), rest)


def my_min(it, **kwargs):
    return ("my_min", list(it), sorted(kwargs.items()))


def use_global():
    return sum(x for x in xs)


def use_global_start():
    return sum((x for x in xs), 5)


def use_global_min():
    return min((x for x in xs), default=0)


def use_local():
    sum = my_sum
    return sum(x for x in xs)


def use_param(sum):
    return sum(x for x in xs)


def use_closure():
    sum = my_sum

    def inner():
        return sum(x for x in xs)

    return inner()


def use_late_local(flag):
    if flag:
        any = lambda it: ("local any", list(it))
    try:
        return any(x > 8 for x in xs)
    except UnboundLocalError as exc:
        return type(exc).__name__


print(use_global(), use_local(), use_param(my_sum), use_closure())
print(use_param(sum), use_late_local(True), use_late_local(False))

real_sum, real_min = builtins.sum, builtins.min
builtins.sum = my_sum
builtins.min = my_min
print(use_global(), use_global_start(), use_global_min())
builtins.sum = real_sum
builtins.min = real_min
print(use_global(), use_global_start(), use_global_min())

del builtins.sum
try:
    print(use_global())
except NameError as exc:
    print("NameError", exc)
builtins.sum = real_sum
print(use_global())

namespace = {"my_sum": my_sum, "xs": xs}
exec(
    "def probe():\n"
    "    return sum(x for x in xs)\n"
    "before = probe()\n"
    "sum = my_sum\n"
    "during = probe()\n"
    "del sum\n"
    "after = probe()\n",
    namespace,
)
print(namespace["before"], namespace["during"], namespace["after"])

alias = min
print(alias(x for x in xs), (max)(x for x in xs), builtins.sum(x for x in xs))

# A different builtin under the reduction's name is not the reduction.
real_any = builtins.any
builtins.any = all
print(any(x > 1 for x in xs))
builtins.any = real_any
print(any(x > 1 for x in xs))

print("== scopes")


class C:
    vals = [1, 2, 3]
    total = sum(v for v in vals)
    any_big = any(v > 2 for v in vals)
    low = min(v * 2 for v in vals)

    def sum(self):
        return "method"

    after = sum


print(C.total, C.any_big, C.low, C().after())

x = "outer"
print(sum(x for x in range(4)), x)


def walrus():
    total = sum((last := v) for v in range(5))
    return total, last


print(walrus())
print(sum((w := v) for v in range(4)), w)
print(all((seen_last := v) < 2 for v in range(5)), seen_last)


def closure_cells():
    fs = []
    n = sum(fs.append(lambda: v) or 1 for v in range(3))
    return n, [f() for f in fs]


print(closure_cells())


def free_vars(scale):
    def inner(data):
        return sum(v * scale for v in data), max(v + scale for v in data)

    return inner


print(free_vars(10)([1, 2, 3]))


def nested():
    return sum(sum(a * b for b in range(a)) for a in range(5))


print(nested(), max(min(c for c in row) for row in [[3, 1], [5, 4]]))
print(sum(v for v in (max(r) for r in [[1, 2], [3, 0]])))
print((lambda data: any(v is None for v in data))([1, None]))
print([sum(v for v in range(n)) for n in range(5)])
print({n: max(v for v in range(n + 1)) for n in range(3)})


def recursive(n):
    return n if n < 2 else sum(recursive(n - k) for k in (1, 2))


print(recursive(10))


def gen_user():
    # A reduction inside a generator function.
    for n in range(3):
        yield sum(v for v in range(n)), all(v < 1 for v in range(n))


print(list(gen_user()))


def unbound_free():
    def inner():
        return sum(v + later for v in [1])

    try:
        return inner()
    except NameError as exc:
        return type(exc).__name__, str(exc)
    later = 1


print(unbound_free())

print("== frame identity")
print(sum(len(sys._getframe().f_code.co_name) for _ in range(2)))
print(sorted(set().union(*(set(locals()) for q in range(1)))))
print(any(sys._getframe().f_back.f_code.co_name == "<module>" for _ in range(1)))


def qualnames():
    return max(sys._getframe().f_code.co_qualname for _ in range(1))


print(qualnames())

print("== async")
import asyncio


async def answer(v):
    return v


async def async_cases():
    out = []
    out.append(sum(v for v in [await answer(1), 2]))
    try:
        sum(await answer(v) for v in [1, 2])
    except TypeError as exc:
        out.append(str(exc))
    try:
        any(v async for v in agen())
    except TypeError as exc:
        out.append(str(exc))
    return out


async def agen():
    yield 1


print(asyncio.run(async_cases()))
