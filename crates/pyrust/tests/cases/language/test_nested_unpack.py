# Parity fixture for nested tuple unpacking on the left side of assignment (#1211)

# Two-level nesting: first element is tuple
(a, b), c = (1, 2), 3
print(a, b, c)

# Two-level nesting: second element is tuple
a, (b, c) = 1, (2, 3)
print(a, b, c)

# Two-level nesting: both elements are tuples
(a, b), (c, d) = (1, 2), (3, 4)
print(a, b, c, d)

# Three-level nesting
(a, (b, c)), d = (1, (2, 3)), 4
print(a, b, c, d)

# Starred inside a nested tuple target
(a, *b), c = [1, 2, 3], 4
print(a, b, c)

# Starred at tail of nested tuple target
(a, b, *c), d = [1, 2, 3, 4], 5
print(a, b, c, d)

# Nested unpack from a function call result
def pair():
    return (10, 20), 30

(x, y), z = pair()
print(x, y, z)

# Chained assignment with nested lhs
(a, b), c = x = (1, 2), 3
print(a, b, c)
print(x)

# Too many values in nested target
try:
    (a, b), c = (1, 2, 3), 4
except ValueError as e:
    print(type(e).__name__ + ":", e)

# Too few values in nested target
try:
    (a, b, c), d = (1, 2), 3
except ValueError as e:
    print(type(e).__name__ + ":", e)

print("nested unpack OK")
