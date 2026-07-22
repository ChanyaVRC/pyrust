# Positional-splat expansion calls `f(<pos…>, *args[, **kw])` — the CallExArgs
# fast-bind path.  Exercises the fixed-arity fast bind (defaults, keyword
# override, leading positionals, varying splat length → cache re-resolve, list
# vs tuple splat) plus forwarding to a variadic callee (slow path) and the
# binding-error diagnostics.  Exceptions are caught and their messages printed so
# the fixture is caret/traceback-independent and version-stable.


def f(a, b, c, d=10, e=20):
    return (a, b, c, d, e)


# Pure splat, list and tuple, all positional.
print(f(*[1, 2, 3]))
print(f(*(1, 2, 3)))
# Splat + keyword overriding a default.
print(f(*[1, 2, 3], d=99))
print(f(*[1, 2], c=3, e=5))
# Leading positional(s) + splat.
print(f(1, *[2, 3]))
print(f(1, 2, *[3], e=7))
# Splat + **kw.
print(f(*[1, 2], **{"c": 3, "d": 4}))
print(f(1, *[2], **{"c": 3}))
# Empty splat.
print(f(1, 2, 3, *[]))
# Varying splat length across the same call site (cache re-resolve).
for xs in ([1, 2, 3], [1, 2, 3], [0, 0, 0]):
    print(f(*xs))

# Keyword-only parameter reached through a splat.
def k(a, *, b=5):
    return (a, b)


print(k(*[1]))
print(k(*[1], b=9))


# Object identity is preserved through the splat (Rc bump, not a copy).
sentinel = object()


def ident(x):
    return x is sentinel


print(ident(*[sentinel]))


# Forwarding to a variadic callee: `args` must be a FRESH tuple (CPython gives a
# new object, so `args is t` is False), and `kwargs` a fresh dict.
def variadic(*args, **kwargs):
    return (args, sorted(kwargs.items()))


t = (1, 2, 3)
res = variadic(*t, x=1, y=2)
print(res)
print(variadic(*t)[0] is t)  # False


# Binding-error diagnostics must match CPython (these fall to the general binder).
def g(x, y):
    return x + y


for label, call in [
    ("too many", lambda: g(*[1, 2, 3])),
    ("missing", lambda: g(*[1])),
    ("duplicate", lambda: g(1, *[2], x=9)),
    ("unexpected kw", lambda: g(*[1], **{"y": 2, "z": 3})),
]:
    try:
        print(label, call())
    except TypeError as exc:
        print(label, "TypeError:", exc)
