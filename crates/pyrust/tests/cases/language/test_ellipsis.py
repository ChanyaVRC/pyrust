# Ellipsis literal (...) and the `Ellipsis` builtin name — the singleton (PEP 3107)

# Basic repr and str
print(...)          # Ellipsis
print(repr(...))    # Ellipsis
print(str(...))     # Ellipsis

# Type name
print(type(...).__name__)   # ellipsis

# Identity: the Ellipsis singleton is a unique object
x = ...
print(x is ...)     # True
y = ...
print(x is y)       # True

# Truthiness: Ellipsis is truthy
print(bool(...))    # True
if ...:
    print("truthy") # truthy

# Equality
print(... == ...)   # True
print(... != 1)     # True
print(... != None)  # True

# Hashable — hash() returns an integer without raising
print(type(hash(...)).__name__)   # int
print(hash(...) != 0)             # True (Ellipsis never hashes to 0)
print(hash(...) != -1)            # True (-1 is reserved by Python hash protocol)

# Hashable — usable as dict key and set element
d = {...: "val"}
print(d[...])       # val

s = {..., 1, 2}
print(... in s)     # True
print(3 in s)       # False

# In tuple / list / set literals
print((...,))       # (Ellipsis,)
print([...])        # [Ellipsis]

# Nested in data structures
nested = {0: ..., 1: [...]}
print(nested[0])    # Ellipsis
print(nested[1])    # [Ellipsis]

# f-string formatting
print(f"{...}")     # Ellipsis

# Assignment and rebind
a = ...
a = 42
print(a)            # 42 (rebinding a does not mutate the singleton)
print(...)          # Ellipsis (singleton unchanged)

# `Ellipsis` builtin name resolves to the same singleton as the `...` literal
print(Ellipsis)             # Ellipsis
print(... is Ellipsis)      # True
print(type(Ellipsis).__name__)  # ellipsis
print(repr(Ellipsis))       # Ellipsis

# Ellipsis name is usable everywhere the literal is
b = Ellipsis
print(b is ...)             # True
d2 = {Ellipsis: "name"}
print(d2[...])              # name

# Function body as Ellipsis — common stub pattern
def abstract_method(self): ...
print(abstract_method(None))   # None (function returns Ellipsis body value is not used)

class Base:
    def method(self): ...
obj = Base()
print(obj.method())            # None

# Annotated assignment with Ellipsis as value
z: int = ...
print(z)                       # Ellipsis

# Type annotation using Ellipsis (e.g. Callable[..., int] style)
def typed(x: ...): pass

# Subscript with Ellipsis (numpy-style a[...] or Callable[...])
class Subscriptable:
    def __getitem__(self, key): return repr(key)
sub = Subscriptable()
print(sub[...])                # Ellipsis
