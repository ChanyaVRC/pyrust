# Parity fixture for issue #1678: round() ndigits must accept any object
# that implements __index__ (int subclass or custom __index__ method).
# CPython uses operator.index() / _PyLong_AsIndex() to coerce ndigits.

class MyInt(int):
    pass

class Indexable:
    def __index__(self):
        return 2

class IndexReturnsSubclass:
    """__index__ returning an int subclass — CPython accepts with DeprecationWarning."""
    def __index__(self):
        return MyInt(2)

class BadIndex:
    def __index__(self):
        return "oops"

class NoIndex:
    pass

# int subclass as ndigits for float x
print(round(3.14159, MyInt(2)))     # 3.14
print(round(3.14159, MyInt(0)))     # 3.0
print(round(3.14159, MyInt(-1)))    # 0.0
print(round(3.14159, MyInt(5)))     # 3.14159

# int subclass as ndigits for int x
print(round(123456, MyInt(-2)))     # 123500
print(round(1234, MyInt(-2)))       # 1200

# custom __index__ as ndigits
print(round(3.14159, Indexable()))  # 3.14

# __index__ returning an int subclass (CPython accepts, DeprecationWarning in 3.12)
print(round(3.14159, IndexReturnsSubclass()))  # 3.14

# plain int ndigits still works (no regression)
print(round(3.14159, 2))            # 3.14

# no ndigits still works (no regression)
print(round(3.14159))               # 3

# bool is a subclass of int: True == 1, False == 0
print(round(3.14159, True))         # 3.1
print(round(3.14159, False))        # 3.0

# __index__ returning a non-int raises TypeError
try:
    round(3.14159, BadIndex())
except TypeError as e:
    print(e)

# object with no __index__ raises TypeError
try:
    round(3.14159, NoIndex())
except TypeError as e:
    print(e)

# non-indexable literal type raises TypeError
try:
    round(3.14159, "x")
except TypeError as e:
    print(e)
