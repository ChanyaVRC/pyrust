# Parity fixture: bytes() must accept bool items in all iterable paths.
# bool is a subclass of int; True == 1, False == 0.

# Generator expression (exercises the general _ iterable arm)
print(bytes(x for x in [True, False, True]))

# List literal (exercises the List arm)
print(bytes([True, False, 65]))

# Tuple literal (exercises the Tuple arm)
print(bytes((False, True, False)))

# Mix of bool and int in a generator
print(bytes(v for v in [False, 65, True, 66]))

# Out-of-range int after a bool should still raise ValueError
try:
    bytes([True, 256])
except ValueError as e:
    print(f"ValueError: {e}")

# Wrong type after a bool should still raise TypeError
try:
    bytes([True, "x"])
except TypeError as e:
    print(f"TypeError: {e}")

# Regression: plain int iterable still works
print(bytes([65, 66]))

# Regression: bytes(int) still works
print(bytes(3))
