# Issue #1970: `cls.__name__ = "X"` renames the class.
class D:
    pass


D.__name__ = "Renamed"
print(D.__name__)
print(type(D()).__name__)

# __qualname__ is independent of __name__.
print(D.__qualname__)

# Renaming a second time.
D.__name__ = "Again"
print(D.__name__)

# Assigning a non-string raises TypeError with CPython's message.
try:
    D.__name__ = 5
except TypeError as e:
    print(e)

try:
    D.__name__ = ["x"]
except TypeError as e:
    print(e)
