# Issue #2619: bool.__new__(bool, x) applies truthiness conversion and
# returns a canonical bool (True/False), never an int-valued object.

print(repr(bool.__new__(bool)))         # False  (default arg)
print(repr(bool.__new__(bool, 0)))      # False
print(repr(bool.__new__(bool, 5)))      # True
print(repr(bool.__new__(bool, [])))     # False
print(repr(bool.__new__(bool, [1])))    # True
print(repr(bool.__new__(bool, "")))     # False
print(repr(bool.__new__(bool, "x")))    # True
print(repr(bool.__new__(bool, None)))   # False

# the result is a real bool
print(type(bool.__new__(bool, 5)).__name__)   # bool
print(bool.__new__(bool, 5) is True)          # True
print(bool.__new__(bool, 0) is False)         # True

# bool.__new__ is distinct from int.__new__ (bool has its own slot)
print(bool.__new__ is int.__new__)            # False
