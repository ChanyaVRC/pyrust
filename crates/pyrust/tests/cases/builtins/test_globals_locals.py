# Issue #389: globals() / locals() built-ins.
#
# Output is deliberately order-independent: we read specific keys from
# the returned dicts rather than printing the dict itself, because
# pyrust populates the module namespace from its own iteration order
# (env values + active script-frame fastlocals) which need not match
# CPython's insertion order across the boilerplate (`__builtins__`,
# `__name__`, etc.).

x = 5
y = "hello"


def f():
    a = 1
    b = "two"
    return locals()


def g():
    return globals()


# globals() at module scope sees user-defined names.
g_dict = globals()
print("x in globals:", "x" in g_dict)
print("y in globals:", "y" in g_dict)
print("globals['x']:", g_dict["x"])
print("globals['y']:", g_dict["y"])
print("globals is dict:", type(g_dict).__name__)

# globals() from inside a function still returns module globals.
m = g()
print("module x via g():", m["x"])
print("'a' not in module globals:", "a" not in m)

# locals() inside a function returns the function's locals.
loc = f()
print("locals keys (sorted):", sorted(loc.keys()))
print("locals['a']:", loc["a"])
print("locals['b']:", loc["b"])

# Argument validation.
try:
    globals(1)
    print("globals-arg-error: FAIL")
except TypeError:
    print("globals-arg-error: TypeError")

try:
    locals(1)
    print("locals-arg-error: FAIL")
except TypeError:
    print("locals-arg-error: TypeError")


# Issue #483 review: locals() inside a generator body must see the
# generator's own fastlocals, not the caller's frame.  Regression for
# the pre-fix bug where every resume reused the caller's view because
# resume_generator_with_exc bypassed the call-site frame-view push.
def gen_locals():
    g1 = 1
    g2 = "two"
    yield locals()
    g3 = 3
    yield locals()


_gen = gen_locals()
print("gen locals first (sorted):", sorted(next(_gen).keys()))
print("gen locals second (sorted):", sorted(next(_gen).keys()))


# globals() inside a generator still resolves the module namespace.
def gen_globals():
    yield globals()["x"]


print("gen globals x:", next(gen_globals()))
