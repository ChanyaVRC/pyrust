# reversed(bytearray) yields its bytes in reverse as ints, like bytes (#2005).

print(list(reversed(bytearray(b"xy"))))   # [121, 120]
print(list(reversed(bytearray(b"abc"))))  # [99, 98, 97]
print(list(reversed(b"abc")))             # [99, 98, 97]  (bytes unchanged)
print(list(reversed(bytearray(b""))))     # []  (empty)

# Manual stepping + StopIteration.
r = reversed(bytearray(b"abc"))
print(next(r), next(r), next(r))  # 99 98 97
try:
    next(r)
except StopIteration:
    print("StopIteration")

# A genuinely non-reversible object still raises TypeError.
try:
    reversed({1, 2})
except TypeError as e:
    print(type(e).__name__, str(e))  # TypeError 'set' object is not reversible
