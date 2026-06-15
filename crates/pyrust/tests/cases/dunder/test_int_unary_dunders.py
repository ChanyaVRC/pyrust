# Issue #2297: `int` exposes its numeric/unary dunders as callable
# methods, matching CPython 3.12.  `__round__`/`__index__`/`__trunc__`/
# `__floor__`/`__ceil__` were missing (raised AttributeError); the
# `__neg__`/`__pos__`/`__abs__`/`__invert__`/`__divmod__` slots already worked
# and are kept here as a regression guard.


def show(expr, fn):
    try:
        print(expr, "=>", fn())
    except Exception as e:
        print(expr, "=>", type(e).__name__ + ":", e)


# --- bound-method happy path (obj.__dunder__(...)) --------------------------
print((5).__round__())
print((5).__round__(1))
print((125).__round__(-1))
print((155).__round__(-1))
print((-125).__round__(-1))
print((5).__index__())
print((5).__trunc__())
print((7).__floor__())
print((7).__ceil__())
print((-5).__neg__())
print((5).__pos__())
print((-5).__abs__())
print((5).__invert__())
print((5).__divmod__(3))

# --- bool receivers normalise to plain int ---------------------------------
print(True.__index__(), type(True.__index__()).__name__)
print(True.__trunc__(), type(True.__trunc__()).__name__)
print(True.__round__(), type(True.__round__()).__name__)
print(True.__round__(-1))

# --- BigInt receivers ------------------------------------------------------
print((10**40).__index__())
print((10**40).__trunc__())
print((123456789012345678901234567890).__round__(-5))

# --- return types are always int -------------------------------------------
print(type((5).__round__()).__name__, type((5).__round__(1)).__name__)
print(type((5).__index__()).__name__)

# --- unbound descriptor form (int.__dunder__(self, ...)) -------------------
print(int.__round__(5))
print(int.__round__(125, -1))
print(int.__index__(5))
print(int.__trunc__(5))
print(int.__floor__(7))
print(int.__ceil__(7))

# --- descriptor introspection (repr / type) --------------------------------
print(int.__round__, int.__index__, int.__trunc__, int.__floor__, int.__ceil__)
print(bool.__round__, bool.__index__)
print(
    type(int.__round__).__name__,
    type(int.__index__).__name__,
    type(int.__trunc__).__name__,
)

# --- hasattr / dir surface -------------------------------------------------
for n in ["__round__", "__index__", "__trunc__", "__floor__", "__ceil__"]:
    print(n, hasattr(5, n), n in dir(int), n in dir(bool))

# --- arity errors ----------------------------------------------------------
show("(5).__round__(1, 2)", lambda: (5).__round__(1, 2))
show("(5).__index__(1)", lambda: (5).__index__(1))
show("(5).__trunc__(1)", lambda: (5).__trunc__(1))
show("(5).__floor__(1)", lambda: (5).__floor__(1))
show("(5).__ceil__(1)", lambda: (5).__ceil__(1))
show("True.__trunc__(1)", lambda: True.__trunc__(1))

# --- keyword-argument rejection --------------------------------------------
show("(5).__round__(ndigits=1)", lambda: (5).__round__(ndigits=1))
show("True.__round__(ndigits=1)", lambda: True.__round__(ndigits=1))
show("(5).__trunc__(y=1)", lambda: (5).__trunc__(y=1))
show("(5).__index__(x=1)", lambda: (5).__index__(x=1))

# --- wrong-receiver-type errors (unbound descriptor) -----------------------
show("int.__round__('x')", lambda: int.__round__("x"))
show("int.__trunc__('x')", lambda: int.__trunc__("x"))
show("int.__index__(1.5)", lambda: int.__index__(1.5))
show("int.__index__('x')", lambda: int.__index__("x"))

# --- no-receiver descriptor errors -----------------------------------------
show("int.__round__()", lambda: int.__round__())
show("int.__index__()", lambda: int.__index__())
show("int.__trunc__()", lambda: int.__trunc__())
