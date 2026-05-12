# Positional-only parameters (PEP 570)

def f(a, b, /, c):
    return (a, b, c)

# Positional call works
assert f(1, 2, 3) == (1, 2, 3)
assert f(1, 2, c=3) == (1, 2, 3)

# Calling a/b by keyword should raise TypeError
try:
    f(a=1, b=2, c=3)
    print("FAIL: expected TypeError")
except TypeError:
    pass

try:
    f(1, b=2, c=3)
    print("FAIL: expected TypeError")
except TypeError:
    pass

# All positional-only (no normal params after /)
def g(a, b, /):
    return a + b

assert g(1, 2) == 3
try:
    g(a=1, b=2)
    print("FAIL: expected TypeError")
except TypeError:
    pass

# Default values on positional-only
def h(a, b=10, /, c=20):
    return (a, b, c)

assert h(1) == (1, 10, 20)
assert h(1, 2) == (1, 2, 20)
assert h(1, 2, 3) == (1, 2, 3)
assert h(1, 2, c=30) == (1, 2, 30)

# Mixed: positional-only, normal, keyword-only
def k(a, /, b, *, c):
    return (a, b, c)

assert k(1, 2, c=3) == (1, 2, 3)
assert k(1, b=2, c=3) == (1, 2, 3)
try:
    k(a=1, b=2, c=3)
    print("FAIL: expected TypeError")
except TypeError:
    pass

# Positional-only with **kwargs: kwargs catches "a" without overriding.
# This exercises the variadic dispatch path in `apply_call` where
# `param.is_positional_only` is checked in `compute_kw_pos` to *skip*
# binding by keyword and let `**kwargs` absorb the name.
def m(a, /, **kw):
    return (a, kw)

assert m(1) == (1, {})
assert m(1, x=2) == (1, {"x": 2})
# 'a' as a keyword goes into kwargs, doesn't bind to positional-only param
assert m(1, a=99) == (1, {"a": 99})
# Multiple positional-only names collide with kwargs cleanly
def m2(a, b, /, **kw):
    return (a, b, kw)

assert m2(1, 2, a=10, b=20, c=30) == (1, 2, {"a": 10, "b": 20, "c": 30})

# Counterpart: fast path (no *args / **kwargs) raises TypeError for the
# same kind of call. Verifies the two paths do the *right* thing in their
# respective regimes; together they cover the design.
def m3(a, b, /):
    return (a, b)

try:
    m3(1, b=2)
    print("FAIL: fast-path should reject positional-only kw")
except TypeError:
    pass

# Function with *args but no **kwargs: keyword for positional-only still fails
def m4(a, /, *args):
    return (a, args)

assert m4(1, 2, 3) == (1, (2, 3))
try:
    m4(1, a=99)
    print("FAIL: *args path should reject positional-only kw without **kwargs")
except TypeError:
    pass

print("posonly params OK")
