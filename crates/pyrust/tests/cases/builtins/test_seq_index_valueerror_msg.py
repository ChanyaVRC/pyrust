# Parity fixture: list.index / tuple.index ValueError message format.
#
# CPython 3.12 uses different formats for list and tuple:
#   list  → "{repr(x)} is not in list"
#   tuple → "tuple.index(x): x not in tuple"  (no repr)

class Foo:
    def __repr__(self):
        return "Foo(custom)"


# list: missing primitive
try:
    [1, 2].index(99)
except ValueError as e:
    print(str(e))

# list: missing user instance — must call user __repr__
try:
    [1, 2].index(Foo())
except ValueError as e:
    print(str(e))

# tuple: missing primitive — fixed message, no repr
try:
    (1, 2).index(99)
except ValueError as e:
    print(str(e))

# tuple: missing user instance — fixed message, no repr
try:
    (1, 2).index(Foo())
except ValueError as e:
    print(str(e))

# list: missing str — repr must include quotes
try:
    ["a", "b"].index("z")
except ValueError as e:
    print(str(e))

# list: missing float — repr must use float notation
try:
    [1.0, 2.0].index(3.14)
except ValueError as e:
    print(str(e))

# Regression: happy path must still work
print([1, 2, 3].index(2))
print((10, 20, 30).index(20))
