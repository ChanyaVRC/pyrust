# Positional-splat calls with literal `kw=v` keyword arguments after the splat —
# `f(<pos…>, *args, kw=v…)` — the CallExArgs `nkw > 0` path.  Covers fixed-arity
# and variadic callees, positional/keyword collisions, evaluation order, and the
# `f(*a, kw=v, **k)` shape that is intentionally excluded (kept on the generic
# materializing path because a literal keyword and `**k` can name the same key). Exceptions are
# caught and printed so the fixture is caret/traceback-independent.


def fx(a, b, c, d=10, e=20):
    return (a, b, c, d, e)


def var(*args, **kwargs):
    return (args, sorted(kwargs.items()))


# Fixed-arity callee + literal keywords (list / tuple splat, defaults, leading pos).
print(fx(*[1, 2, 3], d=99))
print(fx(*[1, 2], c=3, e=5))
print(fx(1, *[2, 3], e=7))
print(fx(*(1, 2, 3), d=4, e=8))

# Variadic callee + literal keywords.
print(var(*[1, 2, 3], x=1, y=2))
print(var(*(1, 2), k=9))
print(var(1, *[2, 3], z=5))
print(var(*[], only=1))


# A param filled by both a positional (possibly via the splat) and a literal
# keyword is "got multiple values for argument"; an unknown keyword is
# "unexpected keyword" — both must match CPython.
def g(a, b):
    return (a, b)


for label, call in [
    ("dup a", lambda: g(*[1], a=9)),
    ("dup b via splat", lambda: g(*[1, 2], b=9)),
    ("ok", lambda: g(*[1], b=2)),
    ("unexpected", lambda: g(*[1, 2], zzz=3)),
]:
    try:
        print(label, "->", call())
    except TypeError as exc:
        print(label, "-> TypeError:", exc)


# Variadic callee: literal keyword collides with a positional.
def vc(a, *rest, **kw):
    return (a, rest, sorted(kw.items()))


try:
    print(vc(*[1, 2], a=9))
except TypeError as exc:
    print("vc dup -> TypeError:", exc)
print(vc(*[1, 2, 3], x=1))


# Keyword-only parameter satisfied by a literal keyword through the splat.
def ko(a, *, b=5):
    return (a, b)


print(ko(*[1], b=7))
print(ko(*[1]))


# The excluded `f(*a, kw=v, **k)` shape still works (via the generic path),
# including the cross-source duplicate-key error.
print(var(*[1, 2], x=1, **{"y": 2, "z": 3}))
try:
    print(var(*[1], x=1, **{"x": 2}))
except TypeError as exc:
    print("litkw+**kw dup -> TypeError:", exc)


# Evaluation order: positionals, then the `*args` expression, then the literal
# keyword values — strictly left-to-right, matching CPython.
def trace(tag, val):
    print("eval", tag)
    return val


def sink(*a, **k):
    return (a, sorted(k.items()))


print(sink(trace("p0", 0), *trace("splat", [1, 2]), kw=trace("kw", 9)))


# Evaluation order when the splat is a GENERATOR whose ITERATION has side effects:
# with a leading positional, CPython materialises (iterates) the splat into the
# positional tuple BEFORE evaluating the literal keyword values (`BUILD_LIST` +
# `LIST_EXTEND` + `LIST_TO_TUPLE`), so `iter 0` / `iter 1` print before `eval kw`.
# With no leading positional CPython instead defers the splat into
# CALL_FUNCTION_EX, so the keyword value is evaluated first.
def side_gen(n):
    for i in range(n):
        print("iter", i)
        yield i


print(sink(trace("lead", 0), *side_gen(2), kw=trace("kw", 9)))
print(sink(*side_gen(2), kw=trace("kw", 9)))
print(sink(trace("lead", 0), trace("lead2", 1), *side_gen(1), a=trace("a", 8), b=trace("b", 9)))
