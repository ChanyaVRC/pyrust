# Parity fixture for slice() keyword-argument rejection (issue #848).
#
# CPython 3.12: slice() is positional-only.  Any keyword argument raises
# TypeError regardless of which keyword name was used.

# --- keyword argument rejection ---

try:
    slice(stop=5)
except TypeError as e:
    print(type(e).__name__, str(e))

try:
    slice(1, stop=5)
except TypeError as e:
    print(type(e).__name__, str(e))

try:
    slice(start=1, stop=5)
except TypeError as e:
    print(type(e).__name__, str(e))

# --- positional forms still work ---

s1 = slice(5)
print(s1.start, s1.stop, s1.step)

s2 = slice(1, 5)
print(s2.start, s2.stop, s2.step)

s3 = slice(1, 5, 2)
print(s3.start, s3.stop, s3.step)
