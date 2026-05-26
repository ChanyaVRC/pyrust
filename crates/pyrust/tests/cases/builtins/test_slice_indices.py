# slice.indices(length) — basic cases
s = slice(1, 10, 2)
print(s.indices(20))                       # (1, 10, 2)
print(s.indices(5))                        # (1, 5, 2)
print(s.indices(0))                        # (0, 0, 2)

# Default bounds
print(slice(None, None, None).indices(5))  # (0, 5, 1)
print(slice(None, None, -1).indices(5))    # (4, -1, -1)

# Negative indices
print(slice(-3, None).indices(10))         # (7, 10, 1)
print(slice(1, -1).indices(5))             # (1, 4, 1)
print(slice(-1, None, -1).indices(5))      # (4, -1, -1)

# Out-of-bounds clamping
print(slice(100, 200).indices(10))         # (10, 10, 1)
print(slice(-100, None, -1).indices(5))    # (-1, -1, -1)
print(slice(100, None, -1).indices(5))     # (4, -1, -1)

# Empty sequence
print(slice(None, None, -1).indices(0))    # (-1, -1, -1)

# dir() includes 'indices'
print('indices' in dir(slice(1, 2)))       # True

# TypeError for non-integer length
try:
    slice(1, 2).indices('x')
except TypeError as e:
    print(type(e).__name__, e)

# ValueError for zero step
try:
    slice(None, None, 0).indices(5)
except ValueError as e:
    print(type(e).__name__, e)

# ValueError for negative length
try:
    slice(1, 2).indices(-1)
except ValueError as e:
    print(type(e).__name__, e)

# TypeError for non-integer slice bound
try:
    slice(1.5, None).indices(5)
except TypeError as e:
    print(type(e).__name__, e)
