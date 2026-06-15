# Issue #2481: `float` exposes `__trunc__`/`__floor__`/`__ceil__`/`__round__`
# as callable methods, matching CPython 3.12.  The first three raised TypeError
# on a bound call and `__round__` was missing entirely (AttributeError).  Unlike
# the int slots (#2297), the float `__trunc__`/`__floor__`/`__ceil__` round to an
# int toward zero / -inf / +inf (they are not identity), and `float.__round__`
# treats an explicit `ndigits=None` as omitted (the int slot rejects None).


def show(expr, fn):
    try:
        print(expr, "=>", fn())
    except Exception as e:
        print(expr, "=>", type(e).__name__ + ":", e)


# --- bound-method happy path (obj.__dunder__(...)) --------------------------
print((1.7).__trunc__())
print((-1.7).__trunc__())
print((1.7).__floor__())
print((-1.7).__floor__())
print((1.7).__ceil__())
print((-1.7).__ceil__())
print((1.5).__round__())
print((2.5).__round__())
print((1.5).__round__(0))
print((1.7).__round__(1))
print((125.0).__round__(-1))
print((0.0).__round__())
print((-0.0).__round__())

# --- return types ----------------------------------------------------------
# __trunc__/__floor__/__ceil__ always return int; __round__() -> int,
# __round__(n) -> float.
print(type((1.7).__trunc__()).__name__)
print(type((1.7).__floor__()).__name__)
print(type((1.7).__ceil__()).__name__)
print(type((1.5).__round__()).__name__)
print(type((1.5).__round__(0)).__name__)

# --- ndigits=None is "omitted" for float (returns int) ---------------------
# Diverges from int: `(5).__round__(None)` raises, but `(1.7).__round__(None)`
# returns 2 (an int), exactly like `round(1.7, None)`.
print((1.7).__round__(None), type((1.7).__round__(None)).__name__)
print((1.5).__round__(None))

# --- large values promote to BigInt ----------------------------------------
print((1e30).__trunc__())
print((1e30).__floor__())
print((-1e30).__ceil__())

# --- float subclass receivers ----------------------------------------------
class F(float):
    pass


print(F(1.7).__trunc__())
print(F(-1.7).__floor__())
print(F(1.7).__ceil__())
print(F(1.5).__round__())
print(F(1.5).__round__(0))

# --- unbound descriptor form (float.__dunder__(self, ...)) -----------------
print(float.__trunc__(1.7))
print(float.__floor__(-1.7))
print(float.__ceil__(-1.7))
print(float.__round__(1.5))
print(float.__round__(1.5, 0))

# --- descriptor introspection (repr / type) --------------------------------
print(float.__round__, float.__trunc__, float.__floor__, float.__ceil__)
print(
    type(float.__round__).__name__,
    type(float.__trunc__).__name__,
    type(float.__floor__).__name__,
    type(float.__ceil__).__name__,
)

# --- hasattr / dir surface -------------------------------------------------
for n in ["__round__", "__trunc__", "__floor__", "__ceil__"]:
    print(n, hasattr(1.7, n), n in dir(float))

# --- NaN / infinity --------------------------------------------------------
show("float('inf').__trunc__()", lambda: float("inf").__trunc__())
show("float('-inf').__floor__()", lambda: float("-inf").__floor__())
show("float('inf').__ceil__()", lambda: float("inf").__ceil__())
show("float('nan').__floor__()", lambda: float("nan").__floor__())

# --- arity errors ----------------------------------------------------------
show("(1.7).__trunc__(1)", lambda: (1.7).__trunc__(1))
show("(1.7).__floor__(1)", lambda: (1.7).__floor__(1))
show("(1.7).__ceil__(1)", lambda: (1.7).__ceil__(1))
show("(1.7).__round__(1, 2)", lambda: (1.7).__round__(1, 2))

# --- ndigits index-coercion ------------------------------------------------
show("(1.7).__round__(1.5)", lambda: (1.7).__round__(1.5))
show("(1.7).__round__('x')", lambda: (1.7).__round__("x"))

# --- keyword-argument rejection --------------------------------------------
show("(1.7).__round__(ndigits=1)", lambda: (1.7).__round__(ndigits=1))
show("(1.7).__trunc__(x=1)", lambda: (1.7).__trunc__(x=1))

# --- wrong-receiver-type errors (unbound descriptor) -----------------------
show("float.__trunc__('x')", lambda: float.__trunc__("x"))
show("float.__floor__('x')", lambda: float.__floor__("x"))
show("float.__ceil__([])", lambda: float.__ceil__([]))
