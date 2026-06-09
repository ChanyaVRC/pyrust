# PEP 695 (#2250): a bounded/constrained type parameter populates __bound__ and
# __constraints__ on its TypeVar; a bare parameter keeps __bound__ == None and
# __constraints__ == (). Covers def, class, and type-alias paths.

# ── Function: single upper bound ─────────────────────────────────────────────

def f[T: int](x: T) -> T:
    return x

print(f.__type_params__[0].__bound__)        # <class 'int'>
print(f.__type_params__[0].__constraints__)  # ()
print(f(5))                                   # 5

# ── Function: constraint tuple ───────────────────────────────────────────────

def g[T: (int, str)](x):
    return x

print(g.__type_params__[0].__bound__)         # None
print(g.__type_params__[0].__constraints__)   # (<class 'int'>, <class 'str'>)

# ── Function: bare parameter has no bound ────────────────────────────────────

def h[T](x):
    return x

print(h.__type_params__[0].__bound__)         # None
print(h.__type_params__[0].__constraints__)   # ()

# ── Function: bound is an arbitrary expression (subscripted generic) ─────────

def boundexpr[T: list[int]](x):
    return x

print(boundexpr.__type_params__[0].__bound__)  # list[int]

# ── Function: a later bound may reference an earlier parameter ───────────────

def fwd[T, U: T](x):
    return x

print(fwd.__type_params__[1].__bound__.__name__)  # T

# ── Function: mixed bounded / unbounded parameters ───────────────────────────

def mixed[A, B: int, C](x):
    return x

print(mixed.__type_params__[0].__bound__)     # None
print(mixed.__type_params__[1].__bound__)     # <class 'int'>
print(mixed.__type_params__[2].__bound__)     # None

# ── Class: bound and constraints ─────────────────────────────────────────────

class CBound[T: int]:
    pass

print(CBound.__type_params__[0].__bound__)        # <class 'int'>
print(CBound.__type_params__[0].__constraints__)  # ()

class CConstr[T: (int, str)]:
    pass

print(CConstr.__type_params__[0].__bound__)        # None
print(CConstr.__type_params__[0].__constraints__)  # (<class 'int'>, <class 'str'>)

# ── Type alias: bound, constraints, and bare ─────────────────────────────────

type ABound[T: int] = list[T]
print(ABound.__type_params__[0].__bound__)        # <class 'int'>
print(ABound.__type_params__[0].__constraints__)  # ()

type AConstr[T: (int, str)] = list[T]
print(AConstr.__type_params__[0].__bound__)        # None
print(AConstr.__type_params__[0].__constraints__)  # (<class 'int'>, <class 'str'>)

type ABare[T] = list[T]
print(ABare.__type_params__[0].__bound__)          # None
print(ABare.__type_params__[0].__constraints__)    # ()

# ── A parenthesised single element is a 1-tuple of constraints ──────────────

def one[T: (int,)](x):
    return x

print(one.__type_params__[0].__bound__)         # None
print(one.__type_params__[0].__constraints__)   # (<class 'int'>,)

# ── The empty tuple is a (degenerate) bound, not constraints ─────────────────
# CPython leaves __constraints__ == () and sets __bound__ to the empty tuple.

def empty[T: ()](x):
    return x

print(empty.__type_params__[0].__bound__)        # ()
print(empty.__type_params__[0].__constraints__)  # ()
