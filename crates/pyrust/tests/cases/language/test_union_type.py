# PEP 604: union types via `X | Y` syntax.
# Tests both creation, repr, isinstance, issubclass, and __args__.

# Basic creation and repr
t = int | str
print(t)

# isinstance with union
print(isinstance(42, int | str))
print(isinstance("hi", int | str))
print(isinstance(3.14, int | str))
print(isinstance([], int | str))

# issubclass with union
print(issubclass(int, int | str))
print(issubclass(float, int | str))
print(issubclass(bool, int | str))

# __args__ is a tuple of the component types
args = (int | str).__args__
print(type(args).__name__)
print(len(args))
print(args[0] is int)
print(args[1] is str)

# Chaining: left-associative, result is flat
t2 = int | str | float
print(t2)
print(len(t2.__args__))

# (UnionType | type) and (type | UnionType) are flat too
t3 = (int | str) | float
print(t3)
print(len(t3.__args__))

t4 = int | (str | float)
print(t4)
print(len(t4.__args__))

# None is coerced to NoneType in union
print(int | None)
print(None | int)

print(isinstance(None, int | None))
print(isinstance(42, int | None))
print(isinstance("x", int | None))

# Equality is set-based: order doesn't matter
print((int | str) == (str | int))
print((int | str) == (int | float))

# type(int | str).__name__
print(type(int | str).__name__)

# issubclass error for non-class arg 1
try:
    issubclass(42, int | str)
except TypeError as e:
    print("TypeError")

# Deduplication: int | int returns int itself, not a UnionType
t5 = int | int
print(t5 is int)
print(type(t5).__name__)

# Deduplication in chaining: int | str | int has 2 args (not 3)
t6 = int | str | int
print(t6)
print(len(t6.__args__))
