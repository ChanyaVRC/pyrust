# range.__getitem__ parity with CPython 3.12.
# Covers integer index (positive, negative, out-of-bounds, bool) and
# slice subscript (all combinations of step/bounds on forward and backward ranges).

# ── Integer index ─────────────────────────────────────────────────────────────

r = range(10)

# Basic positive indices
print(r[0])    # 0
print(r[3])    # 3
print(r[9])    # 9

# Negative indices (wrap from end)
print(r[-1])   # 9
print(r[-5])   # 5
print(r[-10])  # 0

# IndexError: out of bounds (positive)
try:
    _ = r[10]
except IndexError as e:
    print(type(e).__name__ + ": " + str(e))

# IndexError: out of bounds (negative)
try:
    _ = r[-11]
except IndexError as e:
    print(type(e).__name__ + ": " + str(e))

# IndexError: large positive index
try:
    _ = r[100]
except IndexError as e:
    print(type(e).__name__ + ": " + str(e))

# TypeError: float index
try:
    _ = r[1.5]
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

# TypeError: string index
try:
    _ = r["a"]
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

# Bool index (True == 1, False == 0)
print(r[True])   # 1
print(r[False])  # 0

# Step-2 range
r2 = range(0, 10, 2)
print(r2[0])    # 0
print(r2[2])    # 4
print(r2[4])    # 8
print(r2[-1])   # 8
try:
    _ = r2[5]
except IndexError as e:
    print(type(e).__name__ + ": " + str(e))

# Negative-step range
r3 = range(10, 0, -1)
print(r3[0])    # 10
print(r3[1])    # 9
print(r3[-1])   # 1

# Empty range: every index is out of bounds
r4 = range(0)
try:
    _ = r4[0]
except IndexError as e:
    print(type(e).__name__ + ": " + str(e))

# Single-element range
r5 = range(7, 8)
print(r5[0])    # 7
print(r5[-1])   # 7

# __ __index__ protocol on subscript ─────────────────────────────────────────

class MyIndex:
    def __init__(self, v):
        self.v = v
    def __index__(self):
        return self.v

r = range(10)
print(r[MyIndex(3)])    # 3
print(r[MyIndex(-1)])   # 9

# ── Slice subscript ───────────────────────────────────────────────────────────

r = range(10)

# Basic slices
print(r[2:8])    # range(2, 8)
print(r[0:5])    # range(0, 5)
print(r[5:10])   # range(5, 10)

# Step slices
print(r[::2])    # range(0, 10, 2)
print(r[1::2])   # range(1, 10, 2)
print(r[::-1])   # range(9, -1, -1)
print(r[::3])    # range(0, 10, 3)

# Partial slices
print(r[:5])     # range(0, 5)
print(r[5:])     # range(5, 10)
print(r[:])      # range(0, 10)

# Negative slice bounds
print(r[-5:])    # range(5, 10)
print(r[:-5])    # range(0, 5)
print(r[-3:-1])  # range(7, 9)

# Out-of-bounds slices (clamp, not error)
print(r[0:100])    # range(0, 10)
print(r[-100:100]) # range(0, 10)
print(r[100:200])  # range(10, 10)  — empty

# Empty forward slice (start > stop with default step)
print(r[5:3])    # range(5, 5)

# Negative-step slices on range(10)
print(r[8:2:-1])   # range(8, 2, -1)
print(r[8:2:-2])   # range(8, 2, -2)

# Slice on a step-2 range
r2 = range(0, 10, 2)   # yields [0, 2, 4, 6, 8]
print(r2[1:4])          # range(2, 8, 2)
print(r2[::2])          # range(0, 10, 4)
print(r2[::-1])         # range(8, -2, -2)

# Slice on a negative-step range
r3 = range(10, 0, -1)  # yields [10, 9, 8, 7, 6, 5, 4, 3, 2, 1]
print(r3[2:5])          # range(8, 5, -1)
print(r3[::2])          # range(10, 0, -2)
print(r3[::-1])         # range(1, 11)

# Slice on empty range
r4 = range(0)
print(r4[:])            # range(0, 0)
print(r4[0:5])          # range(0, 0)
