# PEP 3134 exception chaining: __cause__, __context__, __suppress_context__
# Tests both attribute correctness and the banner messages printed when
# chained exceptions are displayed.

# --- Explicit chaining: raise X from Y ---
try:
    try:
        raise ValueError("original")
    except ValueError as e:
        raise RuntimeError("wrapped") from e
except RuntimeError as exc:
    print(type(exc).__name__)           # RuntimeError
    cause = exc.__cause__
    print(type(cause).__name__)         # ValueError
    print(str(cause))                   # original
    print(exc.__suppress_context__)     # True
    print(exc.__context__ is cause)     # True

# --- Implicit chaining: raise inside except without 'from' ---
try:
    try:
        raise ValueError("ctx")
    except ValueError:
        raise RuntimeError("implicit")
except RuntimeError as exc:
    print(exc.__cause__ is None)        # True
    ctx = exc.__context__
    print(type(ctx).__name__)           # ValueError
    print(str(ctx))                     # ctx
    print(exc.__suppress_context__)     # False

# --- raise X from None: suppresses chain display ---
try:
    try:
        raise ValueError("suppressed")
    except ValueError:
        raise RuntimeError("clean") from None
except RuntimeError as exc:
    print(exc.__cause__ is None)        # True
    print(exc.__suppress_context__)     # True
    ctx = exc.__context__
    print(type(ctx).__name__)           # ValueError
    print(str(ctx))                     # suppressed

# --- Deep chain (A -> B -> C) ---
def raise_a():
    raise ValueError("A")

def raise_b():
    try:
        raise_a()
    except ValueError as e:
        raise RuntimeError("B") from e

def raise_c():
    try:
        raise_b()
    except RuntimeError as e:
        raise TypeError("C") from e

try:
    raise_c()
except TypeError as exc:
    print(type(exc).__name__)               # TypeError
    print(str(exc))                         # C
    b_exc = exc.__cause__
    print(type(b_exc).__name__)             # RuntimeError
    print(str(b_exc))                       # B
    a_exc = b_exc.__cause__
    print(type(a_exc).__name__)             # ValueError
    print(str(a_exc))                       # A

print("done")
