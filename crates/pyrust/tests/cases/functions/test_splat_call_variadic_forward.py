# Positional-splat calls `f(<pos…>, *args[, **kw])` forwarding to a VARIADIC
# callee (`*args` / `**kwargs`) — the CallExArgs `ExArgsVariadic` direct-bind
# path.  The callee's `*args` tuple and `**kwargs` dict must be FRESH objects
# (CPython identity), the general binding diagnostics must match, and a
# polymorphic site alternating variadic / fixed-arity callees must not mis-bind.
# Exceptions are caught and printed so the fixture is caret/traceback-independent.


def inner(*args, **kwargs):
    return (args, sorted(kwargs.items()))


# Pure forward: tuple / list / empty splat, with and without **kw.
print(inner(*[1, 2, 3], x=1, y=2))
print(inner(*(1, 2, 3)))
print(inner(*[], k=9))
print(inner(*[1, 2, 3]))

# Identity: `args` is a FRESH tuple (CPython builds a new one), `kwargs` a fresh
# dict — a callee mutating them must not affect the caller's objects.
t = (1, 2, 3)
r = inner(*t)
print(r[0] == t, r[0] is t)  # True, False

d = {"a": 1}


def grab(*a, **k):
    k["injected"] = 99
    return sorted(k.items())


print(grab(*[], **d), sorted(d.items()))  # d must NOT gain 'injected'

# Splatted list mutated after the call must not corrupt the callee's tuple.
lst = [1, 2, 3]
res = inner(*lst)
lst.append(4)
print(res[0])  # (1, 2, 3)


# Leading fixed params + *args; defaults; keyword-only + **kwargs.
def f(a, b, *args):
    return (a, b, args)


print(f(*[1, 2, 3, 4]))
print(f(1, *[2, 3]))
print(f(*[1, 2]))


def g(a, b=5, *args):
    return (a, b, args)


print(g(*[1]))
print(g(*[1, 2, 3]))


def h(a, *, b, **kw):
    return (a, b, sorted(kw.items()))


print(h(*[1], b=2, c=3, d=4))
print(h(*[1], **{"b": 2, "z": 9}))


# **kwargs-only callee: forwarding positionals must raise; *args-only callee:
# forwarding a keyword must raise. Both come from the shared variadic binder.
def konly(**kw):
    return sorted(kw.items())


print(konly(**{"x": 1, "y": 2}))
try:
    konly(*[1, 2])
except TypeError as exc:
    print("TypeError:", exc)


def aonly(*args):
    return args


print(aonly(*[1, 2, 3]))
try:
    aonly(*[1], **{"x": 1})
except TypeError as exc:
    print("TypeError:", exc)


# Recursion forwarding through a variadic callee.
def rec(*a):
    if not a:
        return 0
    return a[0] + rec(*a[1:])


print(rec(*[1, 2, 3, 4, 5]))


# Polymorphic call site: same `fn(*xs)` reaching a variadic then a fixed-arity
# callee must resolve per-callee (no stale ExArgsVariadic bind).
def fixed(a, b, c):
    return a + b + c


for fn in (inner, fixed, inner, fixed):
    print(fn(*[10, 20, 30]))


# A param filled by BOTH a positional (via the splat) and a keyword (via **kw)
# is "got multiple values for argument" — the shared variadic binder must raise
# this exactly like CPython, in GIVEN-keyword order (not param order), ahead of
# unexpected-keyword and missing-arg diagnostics.
def mv(a, b, *rest, **kw):
    return (a, b, rest, sorted(kw.items()))


def fwd(fn, *args, **kwargs):
    return fn(*args, **kwargs)


try:
    fwd(mv, 1, a=9)  # a filled by positional 1 and keyword a=9
except TypeError as exc:
    print("TypeError:", exc)
try:
    fwd(mv, 1, 2, b=9, a=9)  # both collide; keyword order -> report 'b'
except TypeError as exc:
    print("TypeError:", exc)
try:
    fwd(mv, 1, 2, a=9, zzz=3)  # multiple-values beats unexpected-keyword
except TypeError as exc:
    print("TypeError:", exc)
# No collision: 'c' named by keyword is never positionally filled here.
print(fwd(mv, 1, c=9, b=8))
