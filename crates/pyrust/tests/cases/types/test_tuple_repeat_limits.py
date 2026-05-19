# Parity fixture for tuple * int / BigInt repeat (issue #750).
# Tests OverflowError on BigInt operands, MemoryError on very large counts,
# and correct results for normal, zero, and negative repeat counts.

# BigInt * tuple and tuple * BigInt both raise OverflowError (positive).
try:
    _ = (1, 2) * (10**20)
except OverflowError as e:
    print("OverflowError tuple*BigInt:", e)

try:
    _ = (10**20) * (1, 2)
except OverflowError as e:
    print("OverflowError BigInt*tuple:", e)

# Negative BigInt also raises OverflowError (CPython 3.12 behaviour).
try:
    _ = (-10**20) * (1, 2)
except OverflowError as e:
    print("OverflowError -BigInt*tuple:", e)

# Normal repeat.
print((1, 2) * 3)

# Zero count returns empty tuple.
print((1, 2) * 0)

# Negative count returns empty tuple.
print((1, 2) * -1)

# Commutativity: int on the left.
print(3 * (1, 2))

# Large but representable count raises MemoryError rather than aborting.
try:
    _ = (1,) * (2**60)
except MemoryError as e:
    print("MemoryError:", repr(str(e)))
