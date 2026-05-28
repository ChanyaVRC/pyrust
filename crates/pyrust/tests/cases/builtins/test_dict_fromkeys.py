# dict.fromkeys(iterable[, value]) — issue #1390
# Parity fixture for dict.fromkeys classmethod.

# Basic usage — keys from a list, default value None
print(dict.fromkeys(["a", "b", "c"]))

# Explicit value
print(dict.fromkeys(["x", "y"], 0))

# Keys from range
print(dict.fromkeys(range(3)))

# Empty iterable
print(dict.fromkeys([]))

# Return type is dict
print(type(dict.fromkeys([])))

# Keys from a string (iterates characters)
print(dict.fromkeys("abc"))

# Duplicate keys — first occurrence wins for insertion order
print(dict.fromkeys(["a", "b", "a", "c"]))

# Instance call: d.fromkeys(...) uses the same classmethod
d = {}
print(d.fromkeys(["p", "q"], 7))

# "fromkeys" appears in dir({})
print("fromkeys" in dir({}))

# Unhashable key raises TypeError
try:
    dict.fromkeys([[1, 2]])
except TypeError as e:
    print(type(e).__name__, e)

# Too many positional arguments
try:
    dict.fromkeys([1], 2, 3)
except TypeError as e:
    print(type(e).__name__, e)

# Keyword arguments are not accepted
try:
    dict.fromkeys([1], value=0)
except TypeError as e:
    print(type(e).__name__, e)

# Value is shared among all entries (same reference)
v = []
result = dict.fromkeys(["a", "b"], v)
result["a"].append(1)
print(result)

# Keys from a generator
def gen_keys():
    yield "x"
    yield "y"
    yield "z"
print(dict.fromkeys(gen_keys()))

# No arguments raises TypeError
try:
    dict.fromkeys()
except TypeError as e:
    print(type(e).__name__, e)
