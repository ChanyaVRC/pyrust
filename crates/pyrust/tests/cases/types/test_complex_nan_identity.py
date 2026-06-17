# NaN-bearing complex values must stay findable in containers via CPython's
# identity short-circuit (`PyObject_RichCompareBool` checks `a is b` before
# `__eq__`), even though `nan != nan` makes bare `==` False.  See issue #2535.

nan = float("nan")

# Real-part NaN.
z = complex(nan, 0)
print(z in [z])  # True
print(z in (z,))  # True
print({z: 1}[z])  # 1
print(z in {z})  # True
print([z].count(z))  # 1
print([z].index(z))  # 0

m = [z]
m.remove(z)
print(len(m))  # 0

# Imaginary-part NaN.
w = complex(0, nan)
print(w in [w])  # True
print({w: 1}[w])  # 1
print(w in {w})  # True

# Both components NaN.
v = complex(nan, nan)
print(v in [v])  # True
print([v].count(v))  # 1

# Bare `==` on two *distinct* NaN-bearing complex values is False in CPython
# (no identity, and nan != nan).  The container fixes above must NOT leak into
# the `==` operator.
print(complex(nan, 0) == complex(nan, 0))  # False
print(z == z)  # False

# Non-NaN complex is unaffected: ordinary value equality still holds.
c = 1 + 2j
print(c in [c])  # True
print(c in [1 + 2j])  # True  (equal by value, no NaN involved)
print((1 + 2j) == (1 + 2j))  # True
print((3 + 4j) in [1 + 2j, 3 + 4j])  # True
