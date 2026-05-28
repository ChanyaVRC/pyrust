# PEP 604 union type: `None | X` and `X | None` — CPython parity fixture.
#
# CPython rule: `X | Y` creates a types.UnionType only when at least one
# operand is a type (PyClass / BuiltinFunction / UnionType).  `None | None`
# has neither operand as a type and therefore raises TypeError, just like
# `1 | "x"` does.

# --- valid cases: at least one side is a type ---

print(int | None)          # int | None
print(None | int)          # int | None (order preserved)
print(str | None)          # str | None
print(None | str)          # str | None
print(str | int | None)    # str | int | None

# Type of the union object
import sys
t = int | None
# Use the class name rather than repr so we're not sensitive to
# differences in how the type object is displayed.
print(type(t).__name__)    # UnionType

# __args__ reflects both types, with None coerced to NoneType
args = (int | None).__args__
print(int in args)         # True
print(type(None) in args)  # True

# Chained: UnionType | None and None | UnionType
ut = int | str
print(ut | None)           # int | str | None
print(None | ut)           # None | int | str

# --- invalid case: neither operand is a type ---

try:
    None | None
    print("ERROR: should have raised TypeError")
except TypeError as e:
    # Match CPython's exact message
    print(str(e))          # unsupported operand type(s) for |: 'NoneType' and 'NoneType'
