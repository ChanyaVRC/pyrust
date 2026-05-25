# Ellipsis literal (...) — the Ellipsis singleton (PEP 3107 / general Python)

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
