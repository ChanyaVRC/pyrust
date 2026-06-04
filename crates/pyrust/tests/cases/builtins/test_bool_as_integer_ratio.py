# bool.as_integer_ratio() must return an int numerator, not the bool — issue #2113.
# bool is an int subclass, but int.as_integer_ratio returns the plain-int value.

print(True.as_integer_ratio())
print(False.as_integer_ratio())
print(tuple(type(x).__name__ for x in True.as_integer_ratio()))
print(type(True.as_integer_ratio()[0]) is int)
print(type(False.as_integer_ratio()[0]) is int)

# int / BigInt as_integer_ratio() unchanged.
print((5).as_integer_ratio())
print((0).as_integer_ratio())
print((10**30).as_integer_ratio())
