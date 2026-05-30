# PEP 695 generic type alias: `type X[T] = expr`
# Tests that TypeVar objects are created for type parameters and that the RHS
# can reference them, matching CPython 3.12 behaviour.

# ── Basic single type param ───────────────────────────────────────────────────

type Stack[T] = list[T]

print(Stack.__value__)          # list[T]
print(Stack.__type_params__)    # (T,)

T = Stack.__type_params__[0]
print(type(T).__name__)         # TypeVar
print(T.__name__)               # T
print(T.__constraints__)        # ()
print(T.__bound__)              # None

# ── Multiple type params ──────────────────────────────────────────────────────

type Pair[T, U] = tuple[T, U]

print(Pair.__value__)           # tuple[T, U]
print(Pair.__type_params__)     # (T, U)

P_T, P_U = Pair.__type_params__
print(P_T.__name__)             # T
print(P_U.__name__)             # U

# ── Type params do not leak to the enclosing scope ────────────────────────────

type Hidden[X] = list[X]
try:
    print(X)
    print("BAD: X leaked to outer scope")
except NameError:
    print("GOOD: X not in outer scope")

# ── Non-generic alias still works (backward compatibility) ────────────────────

type Vector = list[float]
print(Vector.__name__)          # Vector
print(Vector.__value__)         # list[float]
print(Vector.__type_params__)   # ()

# ── Type alias in function scope ──────────────────────────────────────────────

def make_alias():
    type Inner[FT] = list[FT]
    # FT should not be visible in function scope after the type alias
    try:
        print(FT)
        print("BAD: FT leaked to function scope")
    except NameError:
        print("GOOD: FT not in function scope")
    return Inner

alias = make_alias()
print(alias.__name__)           # Inner
A_T = alias.__type_params__[0]
print(A_T.__name__)             # T
print(alias.__value__)          # list[T]

# ── repr of TypeVar matches CPython ──────────────────────────────────────────

type Sample[K] = dict[K, int]
K_var = Sample.__type_params__[0]
print(K_var.__name__)           # K
# The GenericAlias repr should embed the TypeVar name, not the object address.
print(Sample.__value__)         # dict[K, int]
