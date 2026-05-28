# list.index / tuple.index with BigInt start/stop bounds.
# CPython clamps out-of-range bounds to [0, len] (same as PySlice_AdjustIndices).

# Positive BigInt stop larger than len: clamp to len
lst = [1, 2, 3]
print(lst.index(1, 0, 2**200))   # 0

# Positive BigInt start larger than len: search window is empty → ValueError
try:
    lst.index(2, 2**200)
except ValueError as e:
    print(type(e).__name__, e)

# Negative BigInt start (magnitude > len): clamp to 0
print([1].index(1, -(2**200), 2**200))   # 0

# Both bounds are BigInt extremes
print(lst.index(3, -(2**200), 2**200))   # 2

# BigInt that fits in usize but not i64 (between i64::MAX and usize::MAX)
big = 2**63 + 5
print(lst.index(1, 0, big))   # 0
try:
    lst.index(2, big)
except ValueError as e:
    print(type(e).__name__, e)

# Tuple behaves the same
t = (10, 20, 30)
print(t.index(10, 0, 2**200))   # 0
try:
    t.index(20, 2**200)
except ValueError as e:
    print(type(e).__name__, e)
print(t.index(30, -(2**200), 2**200))   # 2

# Normal (non-BigInt) cases are unaffected
print(lst.index(1, 0, 3))    # 0
print(lst.index(2, 1, 3))    # 1
print(lst.index(3, 2, 3))    # 2
try:
    lst.index(9, 0, 3)
except ValueError as e:
    print(type(e).__name__, e)
