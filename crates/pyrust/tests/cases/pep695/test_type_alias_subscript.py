# PEP 695 TypeAliasType: subscript, __module__, and typing export (issue #2779).
import typing

# ── __module__ and typing.TypeAliasType export ───────────────────────────────
type Vector = list[float]
print(type(Vector).__module__)            # typing
print(hasattr(typing, "TypeAliasType"))   # True
print(type(Vector) is typing.TypeAliasType)  # True
print(typing.TypeAliasType)               # <class 'typing.TypeAliasType'>

from typing import TypeAliasType
print(type(Vector) is TypeAliasType)      # True

# ── Generic alias subscript returns a types.GenericAlias ──────────────────────
type Pair[T] = tuple[T, T]
print(Pair.__type_params__)               # (T,)
result = Pair[int]
print(result)                             # Pair[int]
print(repr(result))                       # Pair[int]
print(type(result))                       # <class 'types.GenericAlias'>
print(result.__origin__ is Pair)          # True
print(result.__args__)                    # (<class 'int'>,)

# Multiple parameters
type Triple[T, U] = tuple[T, U, T]
print(Triple[int, str])                   # Triple[int, str]

# Single-parameter alias
type Single[T] = list[T]
print(Single[int])                        # Single[int]

# Equality of parameterized aliases (GenericAlias __eq__)
print(Pair[int] == Pair[int])             # True
print(Pair[int] == Pair[str])             # False

# ── Slice subscript keeps the (unresolved) slice as the type arg ──────────────
sl = Pair[1:2]
print(sl)                                 # Pair[slice(1, 2, None)]
print(sl.__args__)                        # (slice(1, 2, None),)
print(type(sl).__name__)                  # GenericAlias
print(Pair[1:2:3])                        # Pair[slice(1, 2, 3)]
print(Pair[:])                            # Pair[slice(None, None, None)]

# ── Non-generic alias is not subscriptable ───────────────────────────────────
try:
    Vector[int]
except TypeError as e:
    print("TypeError:", e)  # TypeError: Only generic type aliases are subscriptable

try:
    Vector[1:2]
except TypeError as e:
    print("TypeError:", e)  # TypeError: Only generic type aliases are subscriptable

print("type alias subscript ok")
