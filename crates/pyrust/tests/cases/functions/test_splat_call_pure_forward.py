# The `wrapper(*a, **k) -> inner(*args[, **kw])` pure-variadic-forward direct
# bind (#2852): when the callee's params are exactly a single `*args` plus an
# optional `**kwargs` and nothing else, the CallExArgs handler builds the `*A`
# tuple and `**K` dict DIRECTLY and binds them into the two param registers.
# The forwarding wrapper pins the call site's `ExArgsVariadic { pure_forward }`
# cache, so every case below runs through a warm cache entry — the identity /
# freshness landmines must hold under that cache, and a polymorphic callee must
# not mis-bind.  Exceptions are caught + printed so the fixture is
# caret/traceback-independent.


def inner(*args, **kw):
    return (args, sorted(kw.items()))


def wrapper(*a, **k):
    return inner(*a, **k)


# Warm the site, then exercise varying positional counts + **kw presence.
print(wrapper(1, 2, 3, x=4, y=5))
print(wrapper())
print(wrapper(9))
print(wrapper(z=1))
print(wrapper(*[10, 20], q=1))  # splat from a list, not a tuple


# `*A`-only callee reached via a `**k`-carrying wrapper.
def only_args(*args):
    return args


def wonly(*a, **k):
    return only_args(*a, **k)


print(wonly(1, 2, 3))
print(wonly(*[], **{}))  # empty **{} to *A-only callee is fine
try:
    wonly(1, foo=2)  # non-empty keyword to *A-only -> unexpected keyword
except TypeError as exc:
    print("TypeError:", exc)
try:
    wonly(**{"bar": 3})
except TypeError as exc:
    print("TypeError:", exc)


# Fresh `*A` tuple identity under the warm forward cache.
t = (1, 2, 3)


def wt(*a, **k):
    return only_args(*a, **k)


r = wt(*t)
print(r == t, r is t)  # True, False


# Fresh `**K` dict: a callee mutating kwargs must not touch the caller's dict.
def mut(*args, **kw):
    kw["injected"] = 99
    return sorted(kw.items())


def wmut(*a, **k):
    return mut(*a, **k)


d = {"a": 1}
print(wmut(**d), sorted(d.items()))  # d must NOT gain 'injected'


# Non-str `**kw` key forwarded through the pure-forward path must still raise the
# CPython "keywords must be strings" TypeError (falls back to the slow path).
def wkk(*a, **k):
    return inner(*a, **k)


try:
    wkk(**{1: 2})
except TypeError as exc:
    print("TypeError:", exc)


# Generator callee forwarded through the pure-forward path.
def gen(*args, **kw):
    yield from args
    for key in sorted(kw):
        yield key


def wgen(*a, **k):
    return gen(*a, **k)


print(list(wgen(1, 2, foo=3, bar=4)))


# Recursion through a pure-forward callee (warm cache on the self-forward site).
def rec(*args):
    n = args[0]
    if n == 0:
        return 0
    return n + call_rec(n - 1)


def call_rec(*a, **k):
    return rec(*a, **k)


print(call_rec(5))


# Polymorphic site: the SAME `target(*a, **k)` reaching a variadic (direct-bind)
# then a fixed-arity callee must resolve per-callee, never a stale pure-forward.
def variad(*args, **kw):
    return ("variadic", args, sorted(kw.items()))


def fixed(x, y, z=0):
    return ("fixed", x, y, z)


def poly(target, *a, **k):
    return target(*a, **k)


print(poly(variad, 1, 2, p=3))
print(poly(fixed, 1, 2, z=9))
print(poly(variad, 7))
print(poly(fixed, 4, 5))
