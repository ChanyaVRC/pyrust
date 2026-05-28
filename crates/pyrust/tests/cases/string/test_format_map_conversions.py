# str.format_map: !r, !s, !a conversion flags dispatch user __repr__/__str__.

class MyObj:
    def __repr__(self): return "MyRepr"
    def __str__(self):  return "MyStr"

class NonASCII:
    def __repr__(self): return "Bar(\xe9)"

obj = MyObj()

# !r dispatches __repr__
print("{x!r}".format_map({"x": obj}))

# !s dispatches __str__
print("{x!s}".format_map({"x": obj}))

# !a dispatches __repr__ and ascii-escapes the result
print("{x!a}".format_map({"x": NonASCII()}))

# Primitive types must not regress
print("{x!r}".format_map({"x": 42}))
print("{x!s}".format_map({"x": "hi"}))
print("{x!r}".format_map({"x": "hello"}))
print("{x!a}".format_map({"x": "hi"}))

# No conversion uses __format__ / __str__ (not __repr__)
print("{x}".format_map({"x": obj}))

# Exceptions follow the same dispatch rules
try:
    raise ValueError("oops")
except ValueError as e:
    print("{x!s}".format_map({"x": e}))
    print("{x!r}".format_map({"x": e}))
