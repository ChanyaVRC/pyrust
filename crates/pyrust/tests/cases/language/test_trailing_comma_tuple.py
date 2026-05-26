# Trailing comma on the RHS of an assignment creates a tuple.
# x = 1,   is equivalent to   x = (1,)

# Single-element with trailing comma -> tuple
s1 = 1,
print(s1)
print(type(s1))

s2 = "hello",
print(s2)
print(type(s2))

s3 = None,
print(s3)
print(type(s3))

# Multiple elements with trailing comma -> tuple (regression guard)
t1 = 1, 2,
print(t1)
print(type(t1))

# Multiple elements without trailing comma -> tuple (regression guard)
t2 = 1, 2
print(t2)
print(type(t2))

# No trailing comma, single value -> not a tuple (regression guard)
x = 5
print(x)
print(type(x))

# Parenthesized single-element tuple (unrelated path, regression guard)
p = (1,)
print(p)
print(type(p))

# Chained assignment: both targets receive the tuple
a = b = 1,
print(a)
print(b)
print(type(a))
print(type(b))

# Single-element tuple can be unpacked
(v,) = 1,
print(v)
print(type(v))
