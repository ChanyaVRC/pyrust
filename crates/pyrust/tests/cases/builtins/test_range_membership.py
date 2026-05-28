# Parity fixture for range() membership with bool and BigInt items.
# CPython ref: Objects/rangeobject.c range_contains_long.

# bool (subclass of int): True==1, False==0
print(True in range(2))        # True
print(False in range(2))       # True
print(True in range(1))        # False (range(1) == [0])
print(False in range(0))       # False (empty range)

# negative-step range
print(True in range(5, 0, -1))   # True  (1 is in [5,4,3,2,1])
print(False in range(5, 0, -1))  # False (0 is the exclusive stop)

# regression: plain int must still work
print(0 in range(10))          # True
print(5 in range(10))          # True
print(10 in range(10))         # False

# BigInt that fits in i64 (result of large arithmetic reduced to small value)
big = 2**100 - (2**100 - 5)    # == 5, but may stay as BigInt internally
print(big in range(10))        # True
print(big in range(5))         # False (5 is not in range(5))

# BigInt that cannot fit in any i64-bounded range
huge = 2**200
print(huge in range(10))       # False
