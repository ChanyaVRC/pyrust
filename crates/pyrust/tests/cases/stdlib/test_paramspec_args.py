from typing import ParamSpec, ParamSpecArgs, ParamSpecKwargs, Callable, TypeVar

P = ParamSpec('P')
T = TypeVar('T')

# Type names
print(type(P.args).__name__)     # ParamSpecArgs
print(type(P.kwargs).__name__)   # ParamSpecKwargs

# Repr
print(repr(P.args))    # P.args
print(repr(P.kwargs))  # P.kwargs

# Distinct objects
print(P.args is not P.kwargs)  # True

# __origin__
print(P.args.__origin__ is P)   # True
print(P.kwargs.__origin__ is P)  # True

# Equality semantics
P2 = ParamSpec('P2')
print(P.args == P.args)    # True (same origin)
print(P.args == P2.args)   # False (different origin)
print(P.args == P.kwargs)  # False (different proxy class)

# Importable directly
print(isinstance(P.args, ParamSpecArgs))     # True
print(isinstance(P.kwargs, ParamSpecKwargs)) # True

# Unhashable (defines __eq__ without __hash__, matching CPython)
try:
    hash(P.args)
    print("hashable")
except TypeError:
    print("unhashable")

# Works in annotations (no TypeError)
def logged(f: Callable[P, T]) -> Callable[P, T]:
    def wrapper(*args: P.args, **kwargs: P.kwargs) -> T:
        return f(*args, **kwargs)
    return wrapper

@logged
def add(x: int, y: int) -> int:
    return x + y

print(add(1, 2))  # 3

print("ParamSpec.args/kwargs ok")
